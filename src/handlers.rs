use std::{collections::HashMap};

use wasi_cap_std_sync::Dir;
use hyper::{
    http::header::{HeaderName, HeaderValue},
    http::request::Parts,
    Body, Response, StatusCode,
};
use tracing::{debug};
use wasi_cap_std_sync::WasiCtxBuilder;
use wasmtime::*;
use wasmtime_wasi::*;

use crate::dispatcher::RoutePattern;
use crate::http_util::{internal_error, parse_cgi_headers};
use crate::request::{RequestContext, RequestGlobalContext};

use crate::stream_writer::StreamWriter;
use crate::wasm_module::WasmModuleSource;
use crate::wasm_runner::{prepare_stdio_streams_for_http, prepare_wasm_instance, run_prepared_wasm_instance, WasmLinkOptions};

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
    pub async fn handle_request(
        &self,
        matched_route: &RoutePattern,
        req: &Parts,
        request_body: Vec<u8>,
        request_context: &RequestContext,
        global_context: &RequestGlobalContext,
        logging_key: String,
    ) -> Result<Response<Body>, anyhow::Error> {

        // These broken-out functions are slightly artificial but help solve some lifetime
        // issues (where otherwise you get errors about things not being Send across an
        // await).
        let (stream_writer, instance, store) =
            self.set_up_runtime_environment(matched_route, req, request_body, request_context, global_context, logging_key)?;
        self.spawn_wasm_instance(instance, store, stream_writer.clone());

        let response = compose_response(stream_writer).await?;  // TODO: handle errors

        // TODO: c'mon man
        tokio::time::sleep(tokio::time::Duration::from_micros(1)).await;

        Ok(response)
    }

    fn set_up_runtime_environment(&self, matched_route: &RoutePattern, req: &Parts, request_body: Vec<u8>, request_context: &RequestContext, global_context: &RequestGlobalContext, logging_key: String) -> anyhow::Result<(crate::stream_writer::StreamWriter, Instance, Store<WasiCtx>)> {
        let startup_span = tracing::info_span!("module instantiation").entered();

        let headers = crate::http_util::build_headers(
            matched_route,
            req,
            request_body.len(),
            request_context.client_addr,
            global_context.default_host.as_str(),
            global_context.use_tls,
            &global_context.global_env_vars,
        );

        let stream_writer = crate::stream_writer::StreamWriter::new();
        let redirects = prepare_stdio_streams_for_http(request_body, stream_writer.clone(), global_context, logging_key)?;
        let ctx = self.build_wasi_context_for_request(req, headers, redirects.streams)?;
        let (store, instance) = self.prepare_wasm_instance(global_context, ctx)?;
        
        drop(startup_span);
        
        Ok((stream_writer, instance, store))
    }

    fn spawn_wasm_instance(&self, instance: Instance, store: Store<WasiCtx>, mut stream_writer: StreamWriter) {
        let entrypoint = self.entrypoint.clone();
        let wasm_module_name = self.wasm_module_name.clone();
        
        tokio::spawn(async move {
            match run_prepared_wasm_instance(instance, store, &entrypoint, &wasm_module_name) {
                Ok(()) => stream_writer.done().unwrap(),  // TODO: <--
                Err(e) => tracing::error!("oh no {}", e),  // TODO: behaviour? message? MESSAGE, IVAN?!
            };
        });
    }

    fn build_wasi_context_for_request(&self, req: &Parts, headers: HashMap<String, String>, redirects: crate::wasm_module::IOStreamRedirects<crate::stream_writer::StreamWriter>) -> Result<WasiCtx, Error> {
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
        let link_options = WasmLinkOptions::default()
            .with_http(self.allowed_hosts.clone(), self.http_max_concurrency);
        prepare_wasm_instance(global_context, ctx, &self.wasm_module_source, link_options)
    }
}

pub async fn compose_response(mut stream_writer: StreamWriter) -> anyhow::Result<Response<Body>> {
    let header_block = stream_writer.header_block().await?;
    let mut res = Response::new(Body::wrap_stream(stream_writer.as_stream()));

    let mut sufficient_response = false;
    parse_cgi_headers(String::from_utf8(header_block)?)
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
