use crate::http_util::*;
use crate::runtime::*;

use futures::{Future, FutureExt};
use hyper::{header::HOST, Body, Request, Response};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

mod http_util;
pub mod runtime;
pub mod version;

#[derive(Clone)]
/// A router is responsible for taking an inbound request and sending it to the appropriate handler.
pub struct Router {
    pub module_config: Arc<RwLock<ModuleConfig>>,
    pub cache_config_path: String,
    pub module_config_path: String,
}

impl Router {
    pub fn new(module_config_path: String, cache_config_path: String) -> anyhow::Result<Self> {
        let module_config =
            load_modules_toml(module_config_path.as_str(), cache_config_path.clone())?;
        Ok(Router {
            cache_config_path,
            module_config_path,
            module_config: Arc::new(RwLock::new(module_config)),
        })
    }
    /// Route the request to the correct handler
    ///
    /// Some routes are built in (like healthz), while others are dynamically
    /// dispatched.
    pub fn route(
        &self,
        req: Request<Body>,
        client_addr: SocketAddr,
    ) -> Box<dyn Future<Output = Result<Response<Body>, hyper::Error>> + Unpin + Send> {
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
            "/healthz" => Box::new(async { Ok(Response::new(Body::from("OK"))) }.boxed()),
            "/_reload" => {
                let res = self.reload();
                Box::new(
                    async move {
                        match res {
                            Ok(_) => Ok(Response::new(Body::from("OK"))),
                            Err(e) => Ok(internal_error(&e.to_string())),
                        }
                    }
                    .boxed(),
                )
            }
            _ => match self
                .module_config
                .read()
                .unwrap()
                .handler_for_host_path(host.to_lowercase().as_str(), uri_path)
            {
                Ok(h) => {
                    let cache_config_path = self.cache_config_path.clone();
                    Box::new(
                        async move {
                            Ok::<_, hyper::Error>(
                                h.module
                                    .execute(
                                        h.entrypoint.as_str(),
                                        req,
                                        client_addr,
                                        cache_config_path,
                                    )
                                    .await,
                            )
                        }
                        .boxed(),
                    )
                }
                Err(e) => {
                    log::error!("error: {}", e);
                    Box::new(async { Ok(not_found()) }.boxed())
                }
            },
        }
    }

    /// Reload the config and modules.
    ///
    /// This rebuilds the modules list from config, and then re-initializes all
    /// modules. It will call the `_routes()` method on each module. If caching is
    /// enabled, it will also clear and recreate the module cache.
    fn reload(&self) -> anyhow::Result<()> {
        let new_config = load_modules_toml(
            self.module_config_path.as_str(),
            self.cache_config_path.clone(),
        )?;
        {
            let mut module_config = self.module_config.write().unwrap();
            *module_config = new_config;
            module_config.build_registry(self.cache_config_path.clone())?;
        }

        Ok(())
    }
}

/// Load the configuration TOML
pub fn load_modules_toml(
    filename: &str,
    cache_config_path: String,
) -> Result<ModuleConfig, anyhow::Error> {
    if !Path::new(filename).is_file() {
        return Err(anyhow::anyhow!(
            "no modules configuration file found at {}",
            filename
        ));
    }

    let data = std::fs::read_to_string(filename)?;
    let mut modules: ModuleConfig = toml::from_str(data.as_str())?;

    modules.build_registry(cache_config_path)?;

    Ok(modules)
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
    #[serde(rename = "defaultHost")]
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
    fn build_registry(&mut self, cache_config_path: String) -> anyhow::Result<()> {
        let mut routes = vec![];

        let mut failed_modules: Vec<String> = Vec::new();

        self.modules.iter().for_each(|m| {
            match m.load_routes(cache_config_path.clone()) {
                Err(e) => {
                    // FIXME: I think we could do something better here.
                    failed_modules.push(e.to_string())
                }
                Ok(subroutes) => subroutes
                    .iter()
                    .for_each(|entry| routes.push(Handler::new(entry, m))),
            }
        });

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
            .unwrap_or_else(|| "localhost".to_owned());
        if let Some(routes) = self.route_cache.as_ref() {
            for r in routes {
                // The request must match either the `host` of an entry or the `default_host`
                // for this server.
                match r.host.as_ref() {
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

        Err(anyhow::anyhow!("No handler for {}", uri_fragment))
    }
}

#[cfg(test)]
mod test {

    use super::runtime::Module;
    use super::ModuleConfig;
    #[test]
    fn handler_should_respect_host() {
        let cache = "cache.toml".to_string();

        let module = Module {
            route: "/".to_string(),
            module: "examples/hello.wat".to_owned(),
            volumes: None,
            environment: None,
            entrypoint: None,
            host: None,
        };

        // We should be able to mount the same wasm at a separate route.
        let module2 = Module {
            route: "/".to_string(),
            module: "examples/hello.wasm".to_owned(),
            volumes: None,
            environment: None,
            entrypoint: None,
            host: Some("example.com".to_owned()),
        };

        let mut mc = ModuleConfig {
            modules: vec![module.clone(), module2.clone()],
            route_cache: None,
            default_host: None,
        };

        mc.build_registry(cache).expect("registry build cleanly");

        // This should match a default handler
        let foo = mc
            .handler_for_host_path("localhost", "/")
            .expect("foo.example.com handler found");
        assert!(foo.host.is_none());

        // This should match a handler with host example.com
        let foo = mc
            .handler_for_host_path("example.com", "/")
            .expect("example.com handler found");
        assert!(foo.host.is_some());

        // This should not match any handlers
        assert!(mc.handler_for_host_path("foo.example.com", "/").is_err());
    }
}
