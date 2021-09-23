use std::net::SocketAddr;
use std::{collections::HashMap};
use std::path::{Path};
use std::sync::{Arc, RwLock};

use cap_std::fs::Dir;
use hyper::{
    http::header::{HeaderName, HeaderValue},
    http::request::Parts,
    Body, Request, Response, StatusCode,
};
use sha2::{Digest, Sha256};
use tracing::{debug, instrument};
use wasi_cap_std_sync::WasiCtxBuilder;
use wasi_common::pipe::{ReadPipe, WritePipe};
use wasmtime::*;
use wasmtime_wasi::*;

use crate::emplacer::Bits;
use crate::http_util::{internal_error, not_found, parse_cgi_headers};
use crate::module_loader::Loaded;
use crate::request::{RequestContext, RequestGlobalContext, RequestRouteContext};

use crate::bindle_util::{WagiHandlerInfo};
use crate::wagi_config::{LoadedHandlerConfiguration, ModuleMapConfigurationEntry};
use crate::wasm_module::WasmModuleSource;

const STDERR_FILE: &str = "module.stderr";

#[derive(Clone, Debug)]
pub struct RoutingTable {
    pub entries: Vec<RoutingTableEntry>,
    pub global_context: RequestGlobalContext,
}

#[derive(Clone, Debug)]
pub struct RoutingTableEntry {
    pub route_pattern: RoutePattern,
    pub handler_info: RouteHandler,
    pub handler_name: String,
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

impl RoutingTableEntry {
    pub fn is_match(&self, uri_fragment: &str) -> bool {
        self.route_pattern.is_match(uri_fragment)
    }

    fn build_from_modules_toml(source: &Loaded<ModuleMapConfigurationEntry>) -> anyhow::Result<RoutingTableEntry> {
        let route_pattern = RoutePattern::parse(&source.metadata.route);
        
        let wasm_source = WasmModuleSource::Blob(source.content.clone());
        let wasm_route_handler = WasmRouteHandler {
            wasm_module_source: wasm_source,
            entrypoint: source.metadata.entrypoint.clone().unwrap_or_else(|| DEFAULT_ENTRYPOINT.to_owned()),
            volumes: source.metadata.volumes.clone().unwrap_or_default(),
            allowed_hosts: source.metadata.allowed_hosts.clone(),
            http_max_concurrency: source.metadata.http_max_concurrency,
        };
        let handler_info = RouteHandler::Wasm(wasm_route_handler);

        Ok(Self {
            route_pattern,
            handler_info,
            handler_name: source.metadata.module.clone(),
        })
    }

    fn build_from_bindle_entry(source: &(WagiHandlerInfo, Bits)) -> Option<anyhow::Result<RoutingTableEntry>> {
        let (wagi_handler, bits) = source;

        let route_pattern = RoutePattern::parse(&wagi_handler.route);
        let wasm_source = WasmModuleSource::Blob(bits.wasm_module.clone());
        let wasm_route_handler = WasmRouteHandler {
            wasm_module_source: wasm_source,
            entrypoint: wagi_handler.entrypoint.clone().unwrap_or_else(|| DEFAULT_ENTRYPOINT.to_owned()),
            volumes: bits.volume_mounts.clone(),
            allowed_hosts: wagi_handler.allowed_hosts.clone(),
            http_max_concurrency: None,
        };
        let handler_info = RouteHandler::Wasm(wasm_route_handler);

        Some(Ok(Self {
            route_pattern,
            handler_info,
            handler_name: wagi_handler.parcel.label.name.clone(),
        }))
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
        req: &Parts,
        body: Vec<u8>,
        request_context: &RequestContext,
        global_context: &RequestGlobalContext,
    ) -> Response<Body> {
        match &self.handler_info {
            RouteHandler::HealthCheck => Response::new(Body::from("OK")),
            RouteHandler::Wasm(w) => {
                let route_context = RequestRouteContext { entrypoint: w.entrypoint.clone() };
                let response = w.handle_request(&self, req, body, request_context, &route_context, global_context);
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

        compose_response(redirects.stdout_mutex)
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
    pub fn build(source: &LoadedHandlerConfiguration, global_context: RequestGlobalContext) -> anyhow::Result<RoutingTable> {
        let user_entries = match source {
            LoadedHandlerConfiguration::ModuleMapFile(module_map_entries) =>
                Self::build_from_modules_toml(module_map_entries),
            LoadedHandlerConfiguration::Bindle(bindle_entries) =>
                Self::build_from_bindle_entries(bindle_entries),
        }?;
        let built_in_entries = Self::inbuilt_patterns();

        let entries = user_entries.into_iter().chain(built_in_entries).collect();
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

// TODO: NOT HERE
pub fn compose_response(stdout_mutex: Arc<RwLock<Vec<u8>>>) -> Result<Response<Body>, Error> {
    // Okay, once we get here, all the information we need to send back in the response
    // should be written to the STDOUT buffer. We fetch that, format it, and send
    // it back. In the process, we might need to alter the status code of the result.
    //
    // This is a little janky, but basically we are looping through the output once,
    // looking for the double-newline that distinguishes the headers from the body.
    // The headers can then be parsed separately, while the body can be sent back
    // to the client.

    let out = stdout_mutex.read().unwrap();
    let mut last = 0;
    let mut scan_headers = true;
    let mut buffer: Vec<u8> = Vec::new();
    let mut out_headers: Vec<u8> = Vec::new();
    out.iter().for_each(|i| {
        if scan_headers && *i == 10 && last == 10 {
            out_headers.append(&mut buffer);
            buffer = Vec::new();
            scan_headers = false;
            return; // Consume the linefeed
        }
        last = *i;
        buffer.push(*i)
    });
    let mut res = Response::new(Body::from(buffer));
    let mut sufficient_response = false;
    parse_cgi_headers(String::from_utf8(out_headers)?)
        .iter()
        .for_each(|h| {
            use hyper::header::{CONTENT_TYPE, LOCATION};
            match h.0.to_lowercase().as_str() {
                "content-type" => {
                    sufficient_response = true;
                    res.headers_mut().insert(CONTENT_TYPE, h.1.parse().unwrap());
                }
                "status" => {
                    // The spec does not say that status is a sufficient response.
                    // (It says that it may be added along with Content-Type, because
                    // a status has a content type). However, CGI libraries in the wild
                    // do not set content type correctly if a status is an error.
                    // See https://datatracker.ietf.org/doc/html/rfc3875#section-6.2
                    sufficient_response = true;
                    // Status can be `Status CODE [STRING]`, and we just want the CODE.
                    let status_code = h.1.split_once(' ').map(|(code, _)| code).unwrap_or(h.1);
                    tracing::debug!(status_code, "Raw status code");
                    match status_code.parse::<StatusCode>() {
                        Ok(code) => *res.status_mut() = code,
                        Err(e) => {
                            tracing::log::warn!("Failed to parse code: {}", e);
                            *res.status_mut() = StatusCode::BAD_GATEWAY;
                        }
                    }
                }
                "location" => {
                    sufficient_response = true;
                    res.headers_mut()
                        .insert(LOCATION, HeaderValue::from_str(h.1).unwrap());
                    *res.status_mut() = StatusCode::from_u16(302).unwrap();
                }
                _ => {
                    // If the header can be parsed into a valid HTTP header, it is
                    // added to the headers. Otherwise it is ignored.
                    match HeaderName::from_lowercase(h.0.as_str().to_lowercase().as_bytes()) {
                        Ok(hdr) => {
                            res.headers_mut()
                                .insert(hdr, HeaderValue::from_str(h.1).unwrap());
                        }
                        Err(e) => {
                            tracing::error!(error = %e, header_name = %h.0, "Invalid header name")
                        }
                    }
                }
            }
        });
    if !sufficient_response {
        return Ok(internal_error(
            // Technically, we let `status` be sufficient, but this is more lenient
            // than the specification.
            "Exactly one of 'location' or 'content-type' must be specified",
        ));
    }
    Ok(res)
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
