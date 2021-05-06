use crate::http_util::*;
use crate::runtime::*;

use hyper::{header::HOST, Body, Request, Response};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};

mod http_util;
pub mod runtime;
pub mod version;

/// The default host is 'localhost:3000' because that is the port and host WAGI has used since introduction.
pub const DEFAULT_HOST: &str = "localhost:3000";

#[derive(Clone)]
/// A router is responsible for taking an inbound request and sending it to the appropriate handler.
pub struct Router {
    module_store: ModuleStore,
}

impl Router {
    pub async fn new(
        module_config_path: String,
        cache_config_path: String,
        module_cache: PathBuf,
    ) -> anyhow::Result<Self> {
        let module_config = load_modules_toml(
            &module_config_path,
            cache_config_path.clone(),
            module_cache.clone(),
        )
        .await?;
        let module_store = ModuleStore::new(
            module_config,
            cache_config_path,
            module_config_path,
            module_cache,
        );
        let cloned_store = module_store.clone();
        tokio::spawn(async move { cloned_store.run().await });

        Ok(Router { module_store })
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

        let uri_path = req.uri().path();
        let host = req
            .headers()
            .get(HOST)
            .map(|val| val.to_str().unwrap_or(""))
            .unwrap_or("");
        match uri_path {
            "/healthz" => Ok(Response::new(Body::from("OK"))),
            "/_reload" => {
                self.module_store.reload();
                Ok(Response::new(Body::from("OK")))
            }
            _ => match self
                .module_store
                .handler_for_host_path(host.to_lowercase().as_str(), uri_path)
                .await
            {
                Ok(h) => {
                    let cache_config_path = self.module_store.cache_config_path.clone();
                    let module_cache_dir = self.module_store.module_cache.clone();
                    let res = h
                        .module
                        .execute(
                            h.entrypoint.as_str(),
                            req,
                            client_addr,
                            cache_config_path,
                            module_cache_dir,
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

/// Load the configuration TOML
pub async fn load_modules_toml(
    filename: &str,
    cache_config_path: String,
    module_cache_dir: PathBuf,
) -> Result<ModuleConfig, anyhow::Error> {
    if !Path::new(filename).is_file() {
        return Err(anyhow::anyhow!(
            "no modules configuration file found at {}",
            filename
        ));
    }

    let data = std::fs::read_to_string(filename)?;
    let mut modules: ModuleConfig = toml::from_str(data.as_str())?;

    modules
        .build_registry(cache_config_path, module_cache_dir)
        .await?;

    Ok(modules)
}

#[derive(Clone)]
struct ModuleStore {
    module_config: Arc<RwLock<ModuleConfig>>,
    cache_config_path: String,
    module_config_path: String,
    notify: Arc<Notify>,
    module_cache: PathBuf,
}

impl ModuleStore {
    fn new(
        config: ModuleConfig,
        cache_config_path: String,
        module_config_path: String,
        module_cache: PathBuf,
    ) -> Self {
        ModuleStore {
            module_config: Arc::new(RwLock::new(config)),
            cache_config_path,
            module_config_path,
            notify: Arc::new(Notify::new()),
            module_cache,
        }
    }

    async fn run(&self) {
        loop {
            self.notify.notified().await;
            log::debug!("Reloading module configuration");
            let new_config = match load_modules_toml(
                self.module_config_path.as_str(),
                self.cache_config_path.clone(),
                self.module_cache.clone(),
            )
            .await
            {
                Ok(conf) => conf,
                Err(e) => {
                    log::error!("Error when loading modules, will retry: {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    self.notify.notify_one();
                    continue;
                }
            };
            {
                let mut module_config = self.module_config.write().await;
                *module_config = new_config;
                if let Err(e) = module_config
                    .build_registry(self.cache_config_path.clone(), self.module_cache.clone())
                    .await
                {
                    log::error!("Reload: {}", e);
                }
            }
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

    fn reload(&self) {
        self.notify.notify_one()
    }
}

/// The configuration for all modules in a WAGI site
#[derive(Clone, Deserialize)]
pub struct ModuleConfig {
    /// The default hostname to use if none is supplied.
    ///
    /// If this is not set, the default hostname is `localhost`.
    ///
    /// Incoming HTTP requests MUST match a host name, or else they will not be processed.
    /// That is, the `HOST` field of an HTTP 1.1 request must match either the default
    /// host name specified in this paramter or match the `host` field on the module
    /// that matches this request's path.
    default_host: Option<String>,

    /// this line de-serializes [[module]] as modules
    #[serde(rename = "module")]
    pub modules: Vec<crate::runtime::Module>,

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
        cache_config_path: String,
        module_cache_dir: PathBuf,
    ) -> anyhow::Result<()> {
        let mut routes = vec![];

        let mut failed_modules: Vec<String> = Vec::new();

        for m in self.modules.iter().cloned() {
            let cccp = cache_config_path.clone();
            let module = m.clone();
            let mcd = module_cache_dir.clone();
            let res = tokio::task::spawn_blocking(move || module.load_routes(cccp, mcd)).await?;
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
        let default_host = self
            .default_host
            .clone()
            .unwrap_or_else(|| DEFAULT_HOST.to_owned());
        if let Some(routes) = self.route_cache.as_ref() {
            for r in routes {
                // The request must match either the `host` of an entry or the `default_host`
                // for this server.
                match r.host() {
                    // Host doesn't match. Skip.
                    Some(h) if h != host => continue,
                    // This is not the default domain. Skip.
                    None if default_host != host => continue,
                    // Something matched, so continue our checks.
                    _ => {}
                }
                // The important detail here is that strip_suffix returns None if the suffix
                // does not exist. So ONLY paths that end with /... are substring-matched.
                if r.path
                    .strip_suffix("/...")
                    .map(|i| {
                        log::info!("Comparing {} to {}", uri_fragment.clone(), r.path.as_str());
                        uri_fragment.starts_with(i)
                    })
                    .unwrap_or_else(|| r.path == uri_fragment)
                {
                    return Ok(r.clone());
                }
            }
        }

        Err(anyhow::anyhow!("No handler for //{}{}", host, uri_fragment))
    }
}

#[cfg(test)]
mod test {

    use super::runtime::Module;
    use super::ModuleConfig;
    #[tokio::test]
    async fn handler_should_respect_host() {
        let cache = "cache.toml".to_string();
        let mod_cache = tempfile::tempdir().expect("temp dir created").into_path();

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
            modules: vec![module.clone(), module2.clone()],
            route_cache: None,
            default_host: None,
        };

        mc.build_registry(cache, mod_cache)
            .await
            .expect("registry built cleanly");

        // This should match a default handler
        let foo = mc
            .handler_for_host_path(super::DEFAULT_HOST, "/")
            .expect("foo.example.com handler found");
        assert!(foo.module.host.is_none());

        // This should match a handler with host example.com
        let foo = mc
            .handler_for_host_path("example.com", "/")
            .expect("example.com handler found");
        assert!(foo.module.host.is_some());

        // This should not match any handlers
        assert!(mc.handler_for_host_path("foo.example.com", "/").is_err());
    }
}
