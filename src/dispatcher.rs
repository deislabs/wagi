use std::{collections::HashMap};
use std::path::Path;
use std::sync::{Arc, RwLock};

use cap_std::fs::Dir;
use hyper::{
    http::request::Parts,
    Body, Response,
};
use sha2::{Digest, Sha256};
use tracing::{debug, instrument};
use wasi_cap_std_sync::WasiCtxBuilder;
use wasi_common::pipe::{ReadPipe, WritePipe};
use wasmtime::*;
use wasmtime_wasi::*;

use crate::request::{RequestContext, RequestGlobalContext, RequestRouteContext};

use bindle::Invoice;

use crate::bindle_util::InterestingParcel;
use crate::wagi_config::{HandlerConfiguration, ModuleMapConfiguration, ModuleMapConfigurationEntry};
use crate::wasm_module::WasmModuleSource;

const STDERR_FILE: &str = "module.stderr";

pub struct RoutingTable {
    pub entries: Vec<RoutingTableEntry>,
}

#[derive(Clone, Debug)]
pub struct RoutingTableEntry {
    pub route_pattern: RoutePattern,
    pub handler_info: RouteHandler,
    pub handler_name: String,
}

// TODO: TEMPORARY FOR SERDE
impl Default for RoutingTableEntry {
    fn default() -> Self {
        Self { route_pattern: RoutePattern::parse("/..."), handler_info: RouteHandler::HealthCheck, handler_name: "fake".to_owned() }
    }
}

#[derive(Clone, Debug)]
pub enum RoutePattern {
    Exact(String),
    Prefix(String),
}

#[derive(Clone, Debug)]
pub enum RouteHandler {
    HealthCheck,
    Wasm(WasmRouteHandler),
}

#[derive(Clone, Debug)]
pub struct WasmRouteHandler {
    pub wasm_module_source: WasmModuleSource,
    pub entrypoint: String,
    pub volumes: HashMap<String, String>,
    pub allowed_hosts: Option<Vec<String>>,
    pub http_max_concurrency: Option<u32>,
}

impl RoutingTable {
    #[instrument(level = "trace", skip(self))]
    pub fn route_for(&self, uri_fragment: &str) -> Result<RoutingTableEntry, anyhow::Error> {
        for r in &self.entries {
            tracing::trace!(path = ?r.route_pattern, uri_fragment, "Trying route path");
            if r.is_match(uri_fragment) {
                return Ok(r.clone());
            }
        }

        Err(anyhow::anyhow!("No handler for path {}", uri_fragment))
    }
}

const DEFAULT_ENTRYPOINT: &str = "_start";

const FAKE_WASM_FAKE_FAKE_FAKE: Vec<u8> = vec![];
fn FAKE_VOLUMES_FAKE_FAKE_FAKE() -> HashMap<String, String> { HashMap::new() }

impl RoutingTableEntry {
    pub fn is_match(&self, uri_fragment: &str) -> bool {
        self.route_pattern.is_match(uri_fragment)
    }

    fn build_from_modules_toml(source: &ModuleMapConfigurationEntry) -> anyhow::Result<RoutingTableEntry> {
        let route_pattern = RoutePattern::parse(&source.route);
        
        let wasm_source = WasmModuleSource::Blob(Arc::new(FAKE_WASM_FAKE_FAKE_FAKE));
        let wasm_route_handler = WasmRouteHandler {
            wasm_module_source: wasm_source,
            entrypoint: source.entrypoint.clone().unwrap_or_else(|| DEFAULT_ENTRYPOINT.to_owned()),
            volumes: FAKE_VOLUMES_FAKE_FAKE_FAKE(),  // TODO
            allowed_hosts: source.allowed_hosts.clone(),
            http_max_concurrency: source.http_max_concurrency,
        };
        let handler_info = RouteHandler::Wasm(wasm_route_handler);

        Ok(Self {
            route_pattern,
            handler_info,
            handler_name: source.module.clone(),
        })
    }

    fn build_from_parcel(source: &InterestingParcel) -> Option<anyhow::Result<RoutingTableEntry>> {
        match source {
            InterestingParcel::WagiHandler(wagi_handler) => {
                let route_pattern = RoutePattern::parse(&wagi_handler.route);
                let wasm_source = WasmModuleSource::Blob(Arc::new(FAKE_WASM_FAKE_FAKE_FAKE));
                let wasm_route_handler = WasmRouteHandler {
                    wasm_module_source: wasm_source,
                    entrypoint: wagi_handler.entrypoint.clone().unwrap_or_else(|| DEFAULT_ENTRYPOINT.to_owned()),
                    volumes: FAKE_VOLUMES_FAKE_FAKE_FAKE(),  // TODO
                    allowed_hosts: wagi_handler.allowed_hosts.clone(),
                    http_max_concurrency: None,
                };
                let handler_info = RouteHandler::Wasm(wasm_route_handler);

                Some(Ok(Self {
                    route_pattern,
                    handler_info,
                    handler_name: source.parcel().label.name.clone(),
                }))
            },
        }
    }

    fn inbuilt(path: &str, handler: RouteHandler) -> Self {
        Self {
            route_pattern: RoutePattern::Exact(path.to_owned()),
            handler_info: handler,
            handler_name: format!("inbuilt:{}", path),
        }
    }

    /// Returns a unique ID for the routing table entry.
    ///
    /// This is the SHA256 sum of the following data, written into the hasher in the following order
    /// (skipping any `None`s):
    ///
    /// - route
    /// - host
    //
    // TODO: this ^^ is the original comment on Module::id() but it doesn't seem to
    // actually use the host
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
        routing_info: &RoutingTableEntry,
        req: &Parts,
        body: Vec<u8>,
        request_context: &RequestContext,
        route_context: &RequestRouteContext,
        global_context: &RequestGlobalContext,
    ) -> Result<Response<Body>, anyhow::Error> {
        match &self.handler_info {
            RouteHandler::HealthCheck => Ok(Response::new(Body::from("OK"))),
            RouteHandler::Wasm(w) => w.handle_request(routing_info, req, body, request_context, route_context, global_context)
        }
    }
}

impl WasmRouteHandler {
    // TODO: I don't think this rightly belongs here. But
    // reasonable place to at least understand the decomposition and
    // dependencies.
    pub fn handle_request(
        &self,
        routing_info: &RoutingTableEntry,
        req: &Parts,
        body: Vec<u8>,
        request_context: &RequestContext,
        route_context: &RequestRouteContext,
        global_context: &RequestGlobalContext,
    ) -> Result<Response<Body>, anyhow::Error> {
        let startup_span = tracing::info_span!("module instantiation").entered();
        let headers = crate::http_util::build_headers(
            &routing_info.route_pattern,
            req,
            body.len(),
            request_context.client_addr,
            global_context.default_host.as_str(),
            global_context.use_tls,
            &global_context.global_env_vars,
        );

        let redirects = Self::prepare_stdio_streams(body, global_context, routing_info.unique_key())?;

        let ctx = self.build_wasi_context_for_request(req, headers, redirects.streams)?;

        let (store, instance) = self.prepare_wasm_instance(global_context, ctx)?;

        // Drop manually to get instantiation time
        drop(startup_span);

        self.run_prepared_wasm_instance(routing_info, route_context, instance, store)?;

        crate::runtime::compose_response(redirects.stdout_mutex)
    }

    fn prepare_stdio_streams(body: Vec<u8>, global_context: &RequestGlobalContext, handler_id: String) -> Result<crate::wasm_module::IORedirectionInfo, Error> {
        let stdin = ReadPipe::from(body);
        let stdout_buf: Vec<u8> = vec![];
        let stdout_mutex = Arc::new(RwLock::new(stdout_buf));
        let stdout = WritePipe::from_shared(stdout_mutex.clone());
        let log_dir = global_context.base_log_dir.join(handler_id);

        // The spec does not say what to do with STDERR.
        // See specifically sections 4.2 and 6.1 of RFC 3875.
        // Currently, we will attach to wherever logs go.
        tracing::info!(log_dir = %log_dir.display(), "Using log dir");
        std::fs::create_dir_all(&log_dir)?;
        let stderr = cap_std::fs::File::from_std(
            std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(log_dir.join(STDERR_FILE))?,
            ambient_authority(),
        );
        let stderr = wasi_cap_std_sync::file::File::from_cap_std(stderr);

        Ok(crate::wasm_module::IORedirectionInfo {
            streams: crate::wasm_module::IOStreamRedirects {
                stdin,
                stdout,
                stderr,
            },
            stdout_mutex,
        })
    }

    fn build_wasi_context_for_request(&self, req: &Parts, headers: HashMap<String, String>, redirects: crate::wasm_module::IOStreamRedirects) -> Result<WasiCtx, Error> {
        let uri_path = req.uri.path();
        let mut args = vec![uri_path.to_string()];
        req.uri
            .query()
            .map(|q| q.split('&').for_each(|item| args.push(item.to_string())))
            .take();
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut builder = WasiCtxBuilder::new()
            .args(&args)?
            .envs(&headers)?
            .stderr(Box::new(redirects.stderr)) // STDERR goes to the console of the server
            .stdout(Box::new(redirects.stdout)) // STDOUT is sent to a Vec<u8>, which becomes the Body later
            .stdin(Box::new(redirects.stdin));

        for (guest, host) in &self.volumes {
            debug!(%host, %guest, "Mapping volume from host to guest");
            // Try to open the dir or log an error.
            match Dir::open_ambient_dir(host, ambient_authority()) {
                Ok(dir) => {
                    builder = builder.preopened_dir(dir, guest)?;
                }
                Err(e) => tracing::error!(%host, %guest, error = %e, "Error opening directory"),
            };
        }

        let ctx = builder.build();
        Ok(ctx)
    }

    fn prepare_wasm_instance(&self, global_context: &RequestGlobalContext, ctx: WasiCtx) -> Result<(Store<WasiCtx>, Instance), Error> {
        let (mut store, engine) = Self::new_store_and_engine(&global_context.cache_config_path, ctx)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::add_to_linker(&mut linker, |cx| cx)?;

        let http = wasi_experimental_http_wasmtime::HttpCtx::new(
            self.allowed_hosts.clone(),
            self.http_max_concurrency,
        )?;
        http.add_to_linker(&mut linker)?;
        
        let module = self.wasm_module_source.load_module(&store)?;
        let instance = linker.instantiate(&mut store, &module)?;
        Ok((store, instance))
    }

    fn run_prepared_wasm_instance(&self, routing_info: &RoutingTableEntry, route_context: &RequestRouteContext, instance: Instance, mut store: Store<WasiCtx>) -> Result<(), Error> {
        let ep = &route_context.entrypoint;
        let start = instance
            .get_func(&mut store, ep)
            .ok_or_else(|| anyhow::anyhow!("No such function '{}' in {}", &ep, routing_info.handler_name))?;
        tracing::trace!("Calling Wasm entry point");
        start.call(&mut store, &[])?;
        Ok(())
    }

    fn new_store_and_engine(
        cache_config_path: &Path,
        ctx: WasiCtx,
    ) -> Result<(Store<WasiCtx>, Engine), anyhow::Error> {
        let mut config = Config::default();
        if let Ok(p) = std::fs::canonicalize(cache_config_path) {
            config.cache_config_load(p)?;
        };

        let engine = Engine::new(&config)?;
        Ok((Store::new(&engine, ctx), engine))
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
}

impl RoutingTable {
    pub fn build(source: &HandlerConfiguration) -> anyhow::Result<RoutingTable> {
        let user_entries = match source {
            HandlerConfiguration::ModuleMapFile(module_map_configuration) =>
                Self::build_from_modules_toml(module_map_configuration),
            HandlerConfiguration::Bindle(invoice) =>
                Self::build_from_bindle(invoice),
        }?;
        let built_in_entries = Self::inbuilt_patterns();

        let entries = user_entries.into_iter().chain(built_in_entries).collect();
        Ok(Self { entries })
    }

    fn build_from_modules_toml(module_map_configuration: &ModuleMapConfiguration) -> anyhow::Result<Vec<RoutingTableEntry>> {
        // TODO: look for `_routes` function
        module_map_configuration.entries
            .iter()
            .map(|e| RoutingTableEntry::build_from_modules_toml(e))
            .collect()
    }

    fn build_from_bindle(invoice: &Invoice) -> anyhow::Result<Vec<RoutingTableEntry>> {
        // TODO: this is duplication!  We should parse the invoice once into a structure of relevant stuff
        let top = crate::bindle_util::top_modules(invoice);
        let interesting_parcels = top.iter().filter_map(|p| crate::bindle_util::classify_parcel(p));

        interesting_parcels
            .filter_map(|p| RoutingTableEntry::build_from_parcel(&p)).collect()
    }

    fn inbuilt_patterns() -> Vec<RoutingTableEntry> {
        vec![
            RoutingTableEntry::inbuilt("/healthz", RouteHandler::HealthCheck),
        ]
    }
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
