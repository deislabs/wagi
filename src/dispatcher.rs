use std::net::SocketAddr;

use hyper::{
    http::request::Parts,
    Body, Request, Response, StatusCode,
};
use sha2::{Digest, Sha256};
use tracing::{instrument};

use crate::dynamic_route::{DynamicRoutes, interpret_routes};
use crate::emplacer::Bits;
use crate::handlers::{RouteHandler, WasmRouteHandler};
use crate::http_util::{not_found};
use crate::module_loader::Loaded;
use crate::request::{RequestContext, RequestGlobalContext, RequestRouteContext};

use crate::bindle_util::{WagiHandlerInfo};
use crate::wagi_config::{LoadedHandlerConfiguration, ModuleMapConfigurationEntry};
use crate::wasm_module::WasmModuleSource;
use crate::wasm_runner::{RunWasmResult, prepare_stdio_streams, prepare_wasm_instance, run_prepared_wasm_instance_if_present};

#[derive(Clone, Debug)]
pub struct RoutingTable {
    entries: Vec<RoutingTableEntry>,
    global_context: RequestGlobalContext,
}

#[derive(Clone, Debug)]
struct RoutingTableEntry {
    pub route_pattern: RoutePattern,
    pub handler_info: RouteHandler,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RoutePattern {
    Exact(String),
    Prefix(String),
}

impl RoutingTable {
    pub async fn handle_request(
        &self,
        req: Request<Body>,
        client_addr: SocketAddr,
    ) -> Result<Response<Body>, hyper::Error> {
        tracing::trace!("Processing request");

        let uri_path = req.uri().path().to_owned();

        let (parts, body) = req.into_parts();
        let data = hyper::body::to_bytes(body)
            .await
            .unwrap_or_default()
            .to_vec();

        match self.route_for(&uri_path) {
            Ok(rte) => {
                let request_context = RequestContext {
                    client_addr,
                };
                let response = rte.handle_request(&parts, data, &request_context, &self.global_context);
                Ok(response)
            },
            Err(_) => Ok(not_found()),
        }

    }

    #[instrument(level = "trace", skip(self))]
    fn route_for(&self, uri_fragment: &str) -> Result<RoutingTableEntry, anyhow::Error> {
        for r in &self.entries {
            // TODO: I THINK THIS IS WRONG.  The spec says we need to match the *last* pattern
            // if there are multiple matching wildcards (this is mentioned under the docs for
            // the _routes feature).
            tracing::trace!(path = ?r.route_pattern, uri_fragment, "Trying route path");
            if r.is_match(uri_fragment) {
                return Ok(r.clone());
            }
        }

        Err(anyhow::anyhow!("No handler for path {}", uri_fragment))
    }
}

const DEFAULT_ENTRYPOINT: &str = "_start";

impl RoutingTableEntry {
    pub fn is_match(&self, uri_fragment: &str) -> bool {
        self.route_pattern.is_match(uri_fragment)
    }

    fn build_from_modules_toml(source: &Loaded<ModuleMapConfigurationEntry>) -> anyhow::Result<RoutingTableEntry> {
        let route_pattern = RoutePattern::parse(&source.metadata.route);
        
        let wasm_source = WasmModuleSource::Blob(source.content.clone());
        let wasm_route_handler = WasmRouteHandler {
            wasm_module_source: wasm_source,
            wasm_module_name: source.metadata.module.clone(),
            entrypoint: source.metadata.entrypoint.clone().unwrap_or_else(|| DEFAULT_ENTRYPOINT.to_owned()),
            volumes: source.metadata.volumes.clone().unwrap_or_default(),
            allowed_hosts: source.metadata.allowed_hosts.clone(),
            http_max_concurrency: source.metadata.http_max_concurrency,
        };
        let handler_info = RouteHandler::Wasm(wasm_route_handler);

        Ok(Self {
            route_pattern,
            handler_info,
        })
    }

    fn build_from_bindle_entry(source: &(WagiHandlerInfo, Bits)) -> Option<anyhow::Result<RoutingTableEntry>> {
        let (wagi_handler, bits) = source;

        let route_pattern = RoutePattern::parse(&wagi_handler.route);
        let wasm_source = WasmModuleSource::Blob(bits.wasm_module.clone());
        let wasm_route_handler = WasmRouteHandler {
            wasm_module_source: wasm_source,
            wasm_module_name: wagi_handler.parcel.label.name.clone(),
            entrypoint: wagi_handler.entrypoint.clone().unwrap_or_else(|| DEFAULT_ENTRYPOINT.to_owned()),
            volumes: bits.volume_mounts.clone(),
            allowed_hosts: wagi_handler.allowed_hosts.clone(),
            http_max_concurrency: None,
        };
        let handler_info = RouteHandler::Wasm(wasm_route_handler);

        Some(Ok(Self {
            route_pattern,
            handler_info,
        }))
    }

    fn inbuilt(path: &str, handler: RouteHandler) -> Self {
        Self {
            route_pattern: RoutePattern::Exact(path.to_owned()),
            handler_info: handler,
        }
    }

    /// Returns a unique ID for the routing table entry.
    ///
    /// This is the SHA256 sum of the route.
    fn unique_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(&self.route_pattern.original_text());
        format!("{:x}", hasher.finalize())
    }

    // TODO: I don't think this rightly belongs here. But
    // reasonable place to at least understand the decomposition and
    // dependencies.
    pub fn handle_request(
        &self,
        req: &Parts,
        body: Vec<u8>,
        request_context: &RequestContext,
        global_context: &RequestGlobalContext,
    ) -> Response<Body> {
        match &self.handler_info {
            RouteHandler::HealthCheck => Response::new(Body::from("OK")),
            RouteHandler::Wasm(w) => {
                let route_context = RequestRouteContext { entrypoint: w.entrypoint.clone() };
                let response = w.handle_request(&self.route_pattern, req, body, request_context, &route_context, global_context, self.unique_key());
                match response {
                    Ok(res) => res,
                    Err(e) => {
                        tracing::error!(error = %e, "error running WASM module");
                        // A 500 error makes sense here
                        let mut srv_err = Response::default();
                        *srv_err.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                        srv_err
                    }
                }
        
            }
        }
    }
}

impl RoutePattern {
    pub fn parse(path_text: &str) -> Self {
        match path_text.strip_suffix("/...") {
            Some(prefix) => Self::Prefix(prefix.to_owned()),
            None => Self::Exact(path_text.to_owned())
        }
    }

    pub fn is_match(&self, uri_fragment: &str) -> bool {
        match self {
            Self::Exact(path) => path == uri_fragment,
            Self::Prefix(prefix) => uri_fragment.starts_with(prefix),
        }
    }

    pub fn script_name(&self) -> String {
        match self {
            Self::Exact(path) => path.clone(),
            Self::Prefix(prefix) =>
                if prefix.starts_with('/') {
                    prefix.to_owned()
                } else {
                    format!("/{}", prefix)
                }
        }
    }

    pub fn original_text(&self) -> String {
        match self {
            Self::Exact(path) => path.to_owned(),
            Self::Prefix(prefix) => format!("{}/...", prefix),
        }
    }

    /// Resolve a relative path from the end of the matched path to the end of the string.
    ///
    /// For example, if the match is `/foo/...` and the path is `/foo/bar`, it should return `"bar"`,
    /// but if the match is `/foo/bar` and the path is `/foo/bar`, it should return `""`.
    pub fn relative_path(&self, uri_path: &str) -> String {
        let path_base = match self {
            Self::Exact(path) => path,
            Self::Prefix(prefix) => prefix,
        };
        // It is possible that a root path request matching /... returns a None here,
        // so in that case the appropriate return is "".
        uri_path.strip_prefix(path_base).unwrap_or("").to_owned()
    }

    pub fn append(&self, other: &RoutePattern) -> Self {
        match self {
            Self::Exact(path) => other.prepend(path),
            Self::Prefix(prefix) => other.prepend(prefix),
        }
    }

    fn prepend(&self, prefix: &str) -> Self {
        match self {
            Self::Exact(subpath) => Self::Exact(format!("{}{}", prefix, subpath)),
            Self::Prefix(subpath) => Self::Prefix(format!("{}{}", prefix, subpath)),
        }
    }
}

impl RoutingTable {
    pub fn build(source: &LoadedHandlerConfiguration, global_context: RequestGlobalContext) -> anyhow::Result<RoutingTable> {
        let user_entries = match source {
            LoadedHandlerConfiguration::ModuleMapFile(module_map_entries) =>
                Self::build_from_modules_toml(module_map_entries),
            LoadedHandlerConfiguration::Bindle(bindle_entries) =>
                Self::build_from_bindle_entries(bindle_entries),
        }?;
        let full_user_entries = augment_dynamic_routes(user_entries, &global_context)?;

        let built_in_entries = Self::inbuilt_patterns();

        let entries = built_in_entries.into_iter().chain(full_user_entries).collect();
        Ok(Self {
            entries,
            global_context,
        })
    }

    fn build_from_modules_toml(module_map_entries: &Vec<Loaded<ModuleMapConfigurationEntry>>) -> anyhow::Result<Vec<RoutingTableEntry>> {
        // TODO: look for `_routes` function
        module_map_entries
            .iter()
            .map(|e| RoutingTableEntry::build_from_modules_toml(e))
            .collect()
    }

    fn build_from_bindle_entries(bindle_entries: &Vec<(WagiHandlerInfo, Bits)>) -> anyhow::Result<Vec<RoutingTableEntry>> {
        bindle_entries
            .iter()
            .filter_map(|e| RoutingTableEntry::build_from_bindle_entry(e))
            .collect()
    }

    fn inbuilt_patterns() -> Vec<RoutingTableEntry> {
        vec![
            RoutingTableEntry::inbuilt("/healthz", RouteHandler::HealthCheck),
        ]
    }
}

fn augment_dynamic_routes(base_entries: Vec<RoutingTableEntry>, global_context: &RequestGlobalContext) -> anyhow::Result<Vec<RoutingTableEntry>> {
    let results: anyhow::Result<Vec<_>> = base_entries.into_iter().map(|e| augment_one_with_dynamic_routes(e, global_context)).collect();
    let augmented = results?.into_iter().flatten().collect();
    Ok(augmented)
}

fn augment_one_with_dynamic_routes(routing_table_entry: RoutingTableEntry, global_context: &RequestGlobalContext) -> anyhow::Result<Vec<RoutingTableEntry>> {
    match &routing_table_entry.handler_info {
        RouteHandler::Wasm(w) => augment_one_wasm_with_dynamic_routes(&routing_table_entry, w, global_context),
        RouteHandler::HealthCheck => Ok(vec![routing_table_entry]),
    }
}

fn augment_one_wasm_with_dynamic_routes(routing_table_entry: &RoutingTableEntry, wasm_route_handler: &WasmRouteHandler, global_context: &RequestGlobalContext) -> anyhow::Result<Vec<RoutingTableEntry>> {
    let redirects = prepare_stdio_streams(vec![] /* TODO: eww */, global_context, routing_table_entry.unique_key())?;

    let ctx = build_wasi_context_for_dynamic_route_query(redirects.streams);

    let (store, instance) = prepare_wasm_instance(global_context, ctx, &wasm_route_handler.wasm_module_source, |_| Ok(()))?;

    match run_prepared_wasm_instance_if_present(instance, store, "_routes") {
        RunWasmResult::WasmError(e) => return Err(anyhow::Error::from(e)),
        RunWasmResult::EntrypointNotFound => Ok(vec![routing_table_entry.clone()]),
        RunWasmResult::Ok(_) => {
            let out = redirects.stdout_mutex.read().unwrap();
            let dynamic_routes_text = std::str::from_utf8(&*out)?;
            let dynamic_routes = interpret_routes(dynamic_routes_text)?;
        
            let mut dynamic_route_entries = append_all_dynamic_routes(&routing_table_entry, wasm_route_handler, dynamic_routes);
            dynamic_route_entries.reverse();
            dynamic_route_entries.push(routing_table_entry.clone());
            Ok(dynamic_route_entries)
        }
    }
}

fn append_all_dynamic_routes(routing_table_entry: &RoutingTableEntry, wasm_route_handler: &WasmRouteHandler, dynamic_routes: DynamicRoutes) -> Vec<RoutingTableEntry> {
    dynamic_routes.subpath_entrypoints.iter().map(|dr| append_one_dynamic_route(&routing_table_entry, wasm_route_handler, &dr.0, &dr.1)).collect()
}

fn append_one_dynamic_route(routing_table_entry: &RoutingTableEntry, wasm_route_handler: &WasmRouteHandler, dynamic_route_pattern: &RoutePattern, entrypoint: &str) -> RoutingTableEntry {
    let mut subpath_handler = wasm_route_handler.clone();
    subpath_handler.entrypoint = entrypoint.to_owned();
    RoutingTableEntry {
        route_pattern: routing_table_entry.route_pattern.append(dynamic_route_pattern),
        handler_info: RouteHandler::Wasm(subpath_handler),
    }
}

fn build_wasi_context_for_dynamic_route_query(redirects: crate::wasm_module::IOStreamRedirects) -> wasi_common::WasiCtx {
    let builder = wasi_cap_std_sync::WasiCtxBuilder::new()
        .stderr(Box::new(redirects.stderr))
        .stdout(Box::new(redirects.stdout));

    builder.build()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn should_produce_relative_path() {
        let uri_path = "/static/images/icon.png";

        let rp1 = RoutePattern::parse("/static/...");
        assert_eq!("/images/icon.png", rp1.relative_path(uri_path));

        let rp2 = RoutePattern::parse("/static/images/icon.png");
        assert_eq!("", rp2.relative_path(uri_path));

        // According to the spec, if "/" matches "/...", then a single "/" should be set
        let rp3 = RoutePattern::parse("/...");
        assert_eq!("/", rp3.relative_path("/"));

        // According to the spec, if "/" matches the SCRIPT_NAME, then "" should be set
        let rp4 = RoutePattern::parse("/");
        assert_eq!("", rp4.relative_path("/"));

        // As a degenerate case, if the path does not match the prefix,
        // then it should return an empty path because this is not
        // a relative path from the given path. While this is a no-op in
        // current Wagi, conceivably we could some day have to alter this
        // behavior. So this test is a canary for a breaking change.
        let rp5 = RoutePattern::parse("/foo");
        assert_eq!("", rp5.relative_path("/bar"));
    }
}
