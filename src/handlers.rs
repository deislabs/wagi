use std::{collections::HashMap};
use std::path::{Path};
use std::sync::{Arc, RwLock};

use cap_std::fs::Dir;
use hyper::{
    http::header::{HeaderName, HeaderValue},
    http::request::Parts,
    Body, Response, StatusCode,
};
use tracing::{debug};
use wasi_cap_std_sync::WasiCtxBuilder;
use wasi_common::pipe::{ReadPipe, WritePipe};
use wasmtime::*;
use wasmtime_wasi::*;

use crate::dispatcher::RoutePattern;
use crate::http_util::{internal_error, parse_cgi_headers};
use crate::request::{RequestContext, RequestGlobalContext, RequestRouteContext};

use crate::wasm_module::WasmModuleSource;

const STDERR_FILE: &str = "module.stderr";

#[derive(Clone, Debug)]
pub enum RouteHandler {
    HealthCheck,
    Wasm(WasmRouteHandler),
}

#[derive(Clone, Debug)]
pub struct WasmRouteHandler {
    pub wasm_module_source: WasmModuleSource,
    pub wasm_module_name: String,
    pub entrypoint: String,
    pub volumes: HashMap<String, String>,
    pub allowed_hosts: Option<Vec<String>>,
    pub http_max_concurrency: Option<u32>,
}

impl WasmRouteHandler {
    pub fn handle_request(
        &self,
        matched_route: &RoutePattern,
        req: &Parts,
        body: Vec<u8>,
        request_context: &RequestContext,
        route_context: &RequestRouteContext,
        global_context: &RequestGlobalContext,
        logging_key: String,
    ) -> Result<Response<Body>, anyhow::Error> {
        let startup_span = tracing::info_span!("module instantiation").entered();
        let headers = crate::http_util::build_headers(
            matched_route,
            req,
            body.len(),
            request_context.client_addr,
            global_context.default_host.as_str(),
            global_context.use_tls,
            &global_context.global_env_vars,
        );

        let redirects = Self::prepare_stdio_streams(body, global_context, logging_key)?;

        let ctx = self.build_wasi_context_for_request(req, headers, redirects.streams)?;

        let (store, instance) = self.prepare_wasm_instance(global_context, ctx)?;

        // Drop manually to get instantiation time
        drop(startup_span);

        self.run_prepared_wasm_instance(route_context, instance, store)?;

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

    fn run_prepared_wasm_instance(&self, route_context: &RequestRouteContext, instance: Instance, mut store: Store<WasiCtx>) -> Result<(), Error> {
        let ep = &route_context.entrypoint;
        let start = instance
            .get_func(&mut store, ep)
            .ok_or_else(|| anyhow::anyhow!("No such function '{}' in {}", &ep, &self.wasm_module_name))?;
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
