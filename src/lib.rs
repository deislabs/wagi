use crate::http_util::*;
use crate::runtime::*;

use indexmap::IndexSet;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hyper::{header::HOST, Body, Request, Response};
use serde::Deserialize;
use tokio::sync::{Notify, RwLock};

mod http_util;
pub mod runtime;
pub mod version;

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
        }
    }

    /// Route the request to the correct handler
    ///
    /// Some routes are built in (like healthz), while others are dynamically
    /// dispatched.
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

        log::trace!("Processing request to {}", req.uri());

        let uri_path = req.uri().path();
        let host = req
            .headers()
            .get(HOST)
            .map(|val| val.to_str().unwrap_or(""))
            .unwrap_or("");
        match uri_path {
            "/healthz" => Ok(Response::new(Body::from("OK"))),
            _ => match self
                .module_store
                .handler_for_host_path(host.to_lowercase().as_str(), uri_path)
                .await
            {
                Ok(h) => {
                    let res = h
                        .module
                        .execute(
                            h.entrypoint.as_str(),
                            req,
                            client_addr,
                            &self.cache_config_path,
                            &self.module_cache,
                            &self.base_log_dir,
                        )
                        .await;
                    Ok(res)
                }
                Err(e) => {
                    log::error!("error: {}", e);
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
}

impl Default for RouterBuilder {
    fn default() -> Self {
        // NOTE: Because we default to tempdirs, there is the very small chance this could fail, so
        // we just log a warning in that case. If there is a better way to do this, we can change it
        // in the future
        RouterBuilder {
            cache_config_path: PathBuf::from("cache.toml"),
            module_cache_dir: tempfile::tempdir()
                .map_err(|e| {
                    log::warn!(
                        "Error while trying to create temporary directory for module cache: {}",
                        e
                    );
                    e
                })
                .map(|td| td.into_path())
                .unwrap_or_default(),
            base_log_dir: tempfile::tempdir()
                .map_err(|e| {
                    log::warn!(
                        "Error while trying to create temporary directory for logging: {}",
                        e
                    );
                    e
                })
                .map(|td| td.into_path())
                .unwrap_or_default(),
            default_host: String::from("localhost:3000"),
            global_env_vars: HashMap::new(),
        }
    }
}

impl RouterBuilder {
    /// Sets a location for the wasmtime config cache
    pub fn cache_config_path(mut self, cache_config_path: impl AsRef<Path>) -> Self {
        self.cache_config_path = cache_config_path.as_ref().to_owned();
        self
    }

    /// Sets a location for caching downloaded Wasm modules
    pub fn module_cache_dir(mut self, module_cache_dir: impl AsRef<Path>) -> Self {
        self.module_cache_dir = module_cache_dir.as_ref().to_owned();
        self
    }

    /// Sets the base log directory which is used as a location for storing module logs in unique
    /// subdirectories
    pub fn base_log_dir(mut self, base_log_dir: impl AsRef<Path>) -> Self {
        self.base_log_dir = base_log_dir.as_ref().to_owned();
        self
    }

    /// Sets the default host to use for virtual hosting. If this is already set in a
    /// `ModulesConfig`, this will be ignored when the router is built
    pub fn default_host(mut self, host: &str) -> Self {
        self.default_host = host.to_owned();
        self
    }

    pub fn global_env_vars(mut self, vars: HashMap<String, String>) -> Self {
        self.global_env_vars = vars;
        self
    }

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

    /// Build the router, loading the config from a bindle with the given name fetched from the
    /// provided server
    pub async fn build_from_bindle(
        self,
        name: &str,
        bindle_server: &str,
    ) -> anyhow::Result<Router> {
        log::info!("Loading bindle {}", name);
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

    fn build(self, mut config: ModuleConfig) -> Router {
        // Apply the global variables on top of the user defined ones. This will overwrite existing
        // user variables and add any ones that don't exist
        if !self.global_env_vars.is_empty() {
            config.modules = config
                .modules
                .into_iter()
                .map(|mut module| {
                    match module.environment.as_mut() {
                        Some(current) => current.extend(self.global_env_vars.clone()),
                        None => module.environment = Some(self.global_env_vars.clone()),
                    }
                    module
                })
                .collect();
        }

        if config.default_host.is_none() {
            config.default_host = Some(self.default_host);
        }

        let module_store = ModuleStore::new(config);
        Router {
            module_store,
            base_log_dir: self.base_log_dir,
            cache_config_path: self.cache_config_path,
            module_cache: self.module_cache_dir,
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

    async fn handler_for_host_path(
        &self,
        host: &str,
        uri_fragment: &str,
    ) -> anyhow::Result<Handler> {
        self.module_config
            .read()
            .await
            .handler_for_host_path(host, uri_fragment)
    }
}

/// The configuration for all modules in a WAGI site
#[derive(Clone, Debug, Deserialize)]
pub struct ModuleConfig {
    /// The default hostname to use if none is supplied.
    ///
    /// If this is not set, the default hostname is `localhost`.
    ///
    /// Incoming HTTP requests MUST match a host name, or else they will not be processed.
    /// That is, the `HOST` field of an HTTP 1.1 request must match either the default
    /// host name specified in this paramter or match the `host` field on the module
    /// that matches this request's path.
    pub default_host: Option<String>,

    /// this line de-serializes [[module]] as modules
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

    /// Given a URI fragment, find the handler that can execute this.
    fn handler_for_host_path(
        &self,
        host: &str,
        uri_fragment: &str,
    ) -> Result<Handler, anyhow::Error> {
        log::trace!(
            "Module::handler_for_host_path: host={}, url_fragment={}",
            host,
            uri_fragment
        );
        let default_host = self
            .default_host
            .clone()
            .unwrap_or_else(|| DEFAULT_HOST.to_owned());
        if let Some(routes) = self.route_cache.as_ref() {
            for r in routes {
                log::trace!(
                    "Module::handler_for_host_path: trying route host={:?} path={}",
                    r.host(),
                    r.path
                );
                // The request must match either the `host` of an entry or the `default_host`
                // for this server.
                match r.host() {
                    // Host doesn't match. Skip.
                    Some(h) if h != host => {
                        log::trace!("Module::handler_for_host_path: host {} did not match", h);
                        continue;
                    }
                    // This is not the default domain. Skip.
                    None if !is_default_host(default_host.as_str(), host) => {
                        log::trace!(
                            "Module::handler_for_host_path: default host {} did not match",
                            default_host
                        );
                        continue;
                    }
                    // Something matched, so continue our checks.
                    _ => {}
                }
                log::trace!("Module::handler_for_host_path: host matched, examining path");
                // The important detail here is that strip_suffix returns None if the suffix
                // does not exist. So ONLY paths that end with /... are substring-matched.
                let route_match = r
                    .path
                    .strip_suffix("/...")
                    .map(|i| {
                        log::info!("Comparing {} to {}", uri_fragment, r.path.as_str());
                        uri_fragment.starts_with(i)
                    })
                    .unwrap_or_else(|| r.path == uri_fragment);
                if route_match {
                    return Ok(r.clone());
                }
            }
        }

        Err(anyhow::anyhow!("No handler for //{}{}", host, uri_fragment))
    }
}

fn is_default_host(default_host: &str, host: &str) -> bool {
    if default_host.starts_with("localhost:") && host.starts_with("127.0.0.1:") {
        return true;
    }
    default_host == host
}

#[cfg(test)]
mod test {
    use super::runtime::Module;
    use super::ModuleConfig;

    #[tokio::test]
    async fn handler_should_respect_host() {
        let cache = std::path::PathBuf::from("cache.toml");
        let mod_cache = tempfile::tempdir().expect("temp dir created");

        let module = Module {
            route: "/".to_string(),
            module: "examples/hello.wat".to_owned(),
            volumes: None,
            environment: None,
            entrypoint: None,
            host: None,
            bindle_server: None,
            allowed_hosts: None,
        };

        // We should be able to mount the same wasm at a separate route.
        let module2 = Module {
            route: "/".to_string(),
            module: "examples/hello.wasm".to_owned(),
            volumes: None,
            environment: None,
            entrypoint: None,
            host: Some("example.com".to_owned()),
            bindle_server: None,
            allowed_hosts: None,
        };

        let mut mc = ModuleConfig {
            modules: vec![module.clone(), module2.clone()].into_iter().collect(),
            route_cache: None,
            default_host: None,
        };

        let tempdir = tempfile::tempdir().expect("Unable to create tempdir");

        mc.build_registry(&cache, mod_cache.path(), tempdir.path())
            .await
            .expect("registry built cleanly");

        // This should match a default handler
        let default_handler = mc
            .handler_for_host_path(super::DEFAULT_HOST, "/")
            .expect("foo.example.com handler found");
        assert!(default_handler.module.host.is_none());

        // This should match a handler with host example.com
        let example_handler = mc
            .handler_for_host_path("example.com", "/")
            .expect("example.com handler found");
        assert!(example_handler.module.host.is_some());

        // This should not match any handlers
        assert!(mc.handler_for_host_path("foo.example.com", "/").is_err());
    }
}
