use crate::http_util::*;
use crate::runtime::*;
use wagi_config::{HandlerConfigurationSource, WagiConfiguration};

use indexmap::IndexSet;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hyper::{Body, Request, Response};
use serde::Deserialize;
use tokio::sync::{Notify, RwLock};
use tracing::instrument;

use ::bindle::standalone::StandaloneRead;
use ::bindle::Invoice;

mod http_util;
pub mod runtime;
mod tls;
pub mod version;
pub mod wagi_config;
pub mod wagi_server;

/// The default host is 'localhost:3000' because that is the port and host WAGI has used since introduction.
pub const DEFAULT_HOST: &str = "localhost:3000";

#[derive(Clone)]
/// A router is responsible for taking an inbound request and sending it to the appropriate handler.
/// The only way to construct a router is with the [`RouterBuilder`](crate::RouterBuilder)
pub struct Router {
    module_store: ModuleStore,
    base_log_dir: PathBuf,
    cache_config_path: PathBuf,
    module_cache: PathBuf,
    default_host: String,
    use_tls: bool,
    global_env_vars: HashMap<String, String>,
}

impl Router {
    /// Creates a new router builder with empty values. For default values, use
    /// [RouterBuilder::default]
    pub fn builder() -> RouterBuilder {
        RouterBuilder {
            cache_config_path: PathBuf::default(),
            module_cache_dir: PathBuf::default(),
            base_log_dir: PathBuf::default(),
            default_host: String::default(),
            global_env_vars: HashMap::new(),
            use_tls: false,
        }
    }

    pub async fn from_configuration(configuration: &WagiConfiguration) -> anyhow::Result<Self> {
        let builder = RouterBuilder {
            cache_config_path: configuration.wasm_cache_config_file.clone(),
            module_cache_dir: configuration.remote_module_cache_dir.clone(),
            base_log_dir: configuration.log_dir.clone(),
            default_host: configuration.http_configuration.default_hostname.clone(),
            global_env_vars: configuration.env_vars.clone(),
            use_tls: configuration.http_configuration.tls.is_some(),
        };

        let router = match &configuration.handlers {
            HandlerConfigurationSource::StandaloneBindle(bindle_dir, bindle_id) =>
                builder.build_from_standalone_bindle(&bindle_id, bindle_dir).await?,
            HandlerConfigurationSource::RemoteBindle(bindle_server_url, bindle_id) =>
                builder.build_from_bindle(&bindle_id, &bindle_server_url.to_string()).await?,
            HandlerConfigurationSource::ModuleConfigFile(module_config_path) =>
                builder.build_from_modules_toml(&module_config_path).await?,
        };
    
        Ok(router)
    }

    /// Route the request to the correct handler
    ///
    /// Some routes are built in (like healthz), while others are dynamically
    /// dispatched.
    #[instrument(level = "info", skip(self, req), fields(uri = %req.uri()))]
    pub async fn route(
        &self,
        req: Request<Body>,
        client_addr: SocketAddr,
    ) -> Result<Response<Body>, hyper::Error> {
        // TODO: Improve the loading. See issue #3
        //
        // Additionally, we could implement an LRU to cache WASM modules. This would
        // greatly reduce the amount of load time per request. But this would come with two
        // drawbacks: (a) it would be different than CGI, and (b) it would involve a cache
        // clear during debugging, which could be a bit annoying.

        tracing::trace!("Processing request");

        let uri_path = req.uri().path();
        match uri_path {
            "/healthz" => Ok(Response::new(Body::from("OK"))),
            _ => match self.module_store.handler_for_path(uri_path).await {
                Ok(h) => {
                    let info = RouterInfo {
                        entrypoint: h.entrypoint,
                        client_addr: client_addr,
                        cache_config_path: self.cache_config_path.clone(),
                        module_cache_dir: self.module_cache.clone(),
                        base_log_dir: self.base_log_dir.clone(),
                        default_host: self.default_host.to_owned(),
                        use_tls: self.use_tls,
                        env_vars: self.global_env_vars.clone(),
                    };

                    let res = h.module.execute(req, info).await;
                    Ok(res)
                }
                Err(e) => {
                    tracing::error!(error = %e, "error when routing");
                    Ok(not_found())
                }
            },
        }
    }
}

/// A builder for setting up a WAGI router. Created from [Router::builder]
pub struct RouterBuilder {
    cache_config_path: PathBuf,
    module_cache_dir: PathBuf,
    base_log_dir: PathBuf,
    default_host: String,
    global_env_vars: HashMap<String, String>,
    use_tls: bool,
}

impl RouterBuilder {
    /// Build the router, loading the config from a toml file at the given path
    pub async fn build_from_modules_toml(self, path: impl AsRef<Path>) -> anyhow::Result<Router> {
        if !tokio::fs::metadata(&path)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            return Err(anyhow::anyhow!(
                "no modules configuration file found at {}",
                path.as_ref().display()
            ));
        }

        let data = std::fs::read(path)?;
        let mut modules: ModuleConfig = toml::from_slice(&data)?;

        modules
            .build_registry(
                &self.cache_config_path,
                &self.module_cache_dir,
                &self.base_log_dir,
            )
            .await?;

        Ok(self.build(modules))
    }

    /// Build the router, loading the config from a bindle with the given name from a standalone
    /// bindle.
    pub async fn build_from_standalone_bindle(
        self,
        name: &::bindle::Id,
        base_path: impl AsRef<Path>,
    ) -> anyhow::Result<Router> {
        tracing::info!(%name, "Loading standalone bindle");
        let reader = StandaloneRead::new(base_path, name).await?;

        let data = tokio::fs::read(&reader.invoice_file).await?;
        let invoice: Invoice = toml::from_slice(&data)?;

        let cache_dir = self.module_cache_dir.join("_ASSETS");
        let mut mods =
            runtime::bindle::standalone_invoice_to_modules(&invoice, reader.parcel_dir, cache_dir)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("Failed to turn Bindle into module configuration: {}", e)
                })?;

        mods.build_registry(
            &self.cache_config_path,
            &self.module_cache_dir,
            &self.base_log_dir,
        )
        .await?;

        Ok(self.build(mods))
    }

    /// Build the router, loading the config from a bindle with the given name fetched from the
    /// provided server
    pub async fn build_from_bindle(
        self,
        name: &::bindle::Id,
        bindle_server: &str,
    ) -> anyhow::Result<Router> {
        tracing::info!(%name, "Loading bindle");
        let cache_dir = self.module_cache_dir.join("_ASSETS");
        let mut mods = runtime::bindle::bindle_to_modules(name, bindle_server, cache_dir)
            .await
            .map_err(|e| {
                anyhow::anyhow!("Failed to turn Bindle into module configuration: {}", e)
            })?;

        mods.build_registry(
            &self.cache_config_path,
            &self.module_cache_dir,
            &self.base_log_dir,
        )
        .await?;

        Ok(self.build(mods))
    }

    fn build(self, config: ModuleConfig) -> Router {
        let module_store = ModuleStore::new(config);
        Router {
            module_store,
            base_log_dir: self.base_log_dir,
            cache_config_path: self.cache_config_path,
            module_cache: self.module_cache_dir,
            default_host: self.default_host,
            use_tls: self.use_tls,
            global_env_vars: self.global_env_vars,
        }
    }
}

#[derive(Clone)]
struct ModuleStore {
    module_config: Arc<RwLock<ModuleConfig>>,
    notify: Arc<Notify>,
}

impl ModuleStore {
    fn new(config: ModuleConfig) -> Self {
        ModuleStore {
            module_config: Arc::new(RwLock::new(config)),
            notify: Arc::new(Notify::new()),
        }
    }

    async fn handler_for_path(&self, uri_fragment: &str) -> anyhow::Result<Handler> {
        self.module_config
            .read()
            .await
            .handler_for_path(uri_fragment)
    }
}

/// The configuration for all modules in a WAGI site
#[derive(Clone, Debug, Deserialize)]
pub struct ModuleConfig {
    /// De-serialize [[module]] as modules
    #[serde(rename = "module")]
    pub modules: IndexSet<crate::runtime::Module>,

    /// Cache of routes.
    ///
    /// This is built by calling `build_registry`.
    #[serde(skip)]
    route_cache: Option<Vec<Handler>>,
}

impl ModuleConfig {
    /// Construct a registry of all routes.
    async fn build_registry(
        &mut self,
        cache_config_path: &Path,
        module_cache_dir: &Path,
        base_log_dir: &Path,
    ) -> anyhow::Result<()> {
        let mut routes = vec![];

        let mut failed_modules: Vec<String> = Vec::new();

        for m in self.modules.iter().cloned() {
            let cccp = cache_config_path.to_owned();
            let module = m.clone();
            let mcd = module_cache_dir.to_owned();
            let bld = base_log_dir.to_owned();
            let res =
                tokio::task::spawn_blocking(move || module.load_routes(&cccp, &mcd, &bld)).await?;
            match res {
                Err(e) => {
                    // FIXME: I think we could do something better here.
                    failed_modules.push(e.to_string())
                }
                Ok(subroutes) => subroutes
                    .into_iter()
                    .for_each(|entry| routes.push(Handler::new(entry, m.clone()))),
            }
        }

        if !failed_modules.is_empty() {
            let msg = failed_modules.join(", ");
            return Err(anyhow::anyhow!("Not all routes could be built: {}", msg));
        }

        self.route_cache = Some(routes);
        Ok(())
    }

    /// Get a handler for a URI fragment (path) or return an error.
    #[instrument(level = "trace", skip(self))]
    fn handler_for_path(&self, uri_fragment: &str) -> Result<Handler, anyhow::Error> {
        if let Some(routes) = self.route_cache.as_ref() {
            for r in routes {
                tracing::trace!(path = %r.path, "Trying route path");
                // The important detail here is that strip_suffix returns None if the suffix
                // does not exist. So ONLY paths that end with /... are substring-matched.
                let route_match = r
                    .path
                    .strip_suffix("/...")
                    .map(|i| {
                        tracing::trace!(path = %r.path, "Comparing uri fragment to path");
                        uri_fragment.starts_with(i)
                    })
                    .unwrap_or_else(|| r.path == uri_fragment);
                if route_match {
                    return Ok(r.clone());
                }
            }
        }

        Err(anyhow::anyhow!("No handler for path {}", uri_fragment))
    }
}

#[cfg(test)]
mod test {
    use super::runtime::Module;
    use super::ModuleConfig;

    #[tokio::test]
    async fn handler_should_match_path() {
        let cache = std::path::PathBuf::from("cache.toml");
        let mod_cache = tempfile::tempdir().expect("temp dir created");

        let module = Module::new("/".to_string(), "examples/hello.wat".to_owned());
        // We should be able to mount the same wasm at a separate route.
        let module2 = Module::new("/foo".to_string(), "examples/hello.wasm".to_owned());

        let mut mc = ModuleConfig {
            modules: vec![module.clone(), module2.clone()].into_iter().collect(),
            route_cache: None,
        };

        let tempdir = tempfile::tempdir().expect("Unable to create tempdir");

        mc.build_registry(&cache, mod_cache.path(), tempdir.path())
            .await
            .expect("registry built cleanly");

        // This should match a default handler
        let default_handler = mc
            .handler_for_path("/")
            .expect("foo.example.com handler found");
        assert_eq!("examples/hello.wat", default_handler.module.module);

        // This should match a handler with host example.com
        let foo_handler = mc
            .handler_for_path("/foo")
            .expect("example.com handler found");
        assert_eq!("examples/hello.wasm", foo_handler.module.module);

        // This should not match any handlers
        assert!(mc.handler_for_path("/bar").is_err());
    }
}
