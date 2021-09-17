use std::{collections::HashMap, net::SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use cap_std::fs::Dir;
use hyper::HeaderMap;
use hyper::{
    header::HOST,
    http::header::{HeaderName, HeaderValue},
    http::request::Parts,
    Body, Request, Response, StatusCode,
};
use oci_distribution::client::{Client, ClientConfig};
use oci_distribution::secrets::RegistryAuth;
use oci_distribution::Reference;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::{debug, instrument};
use url::Url;
use wasi_cap_std_sync::WasiCtxBuilder;
use wasi_common::pipe::{ReadPipe, WritePipe};
use wasmtime::*;
use wasmtime_wasi::*;
use docker_credential;
use docker_credential::DockerCredential;

use crate::request::{RequestContext, RequestGlobalContext, RequestRouteContext};

#[derive(Clone)]
pub enum WasmModuleSource {
    Blob(Arc<Vec<u8>>),
}

const STDERR_FILE: &str = "module.stderr";

// impl WasmModuleSource {
//     // TODO: this needs to be massively split apart
//     #[allow(clippy::too_many_arguments)]
//     #[instrument(level = "info", skip(self, req, body, request_context, route_context, global_context), fields(uri = %req.uri, /* module = %self.module, */ use_tls = %global_context.use_tls, env = ?global_context.global_env_vars))]
//     fn run_wasm(
//         &self,
//         req: &Parts,
//         body: Vec<u8>,
//         request_context: RequestContext,
//         route_context: RequestRouteContext,
//         global_context: RequestGlobalContext,
//     ) -> Result<Response<Body>, anyhow::Error> {
//         let startup_span = tracing::info_span!("module instantiation").entered();
//         let uri_path = req.uri.path();
//         let headers = self.build_headers(
//             req,
//             body.len(),
//             request_context.client_addr,
//             global_context.default_host.as_str(),
//             global_context.use_tls,
//             global_context.global_env_vars,
//         );
//         let stdin = ReadPipe::from(body);
//         let stdout_buf: Vec<u8> = vec![];
//         let stdout_mutex = Arc::new(RwLock::new(stdout_buf));
//         let stdout = WritePipe::from_shared(stdout_mutex.clone());

//         // Make sure the directory exists
//         let log_dir = global_context.base_log_dir.join(self.id());
//         tracing::info!(log_dir = %log_dir.display(), "Using log dir");
//         std::fs::create_dir_all(&log_dir)?;
//         // Open a file for appending. Right now this will just keep appending as there is no log
//         // rotation or cleanup
//         let stderr = unsafe {
//             cap_std::fs::File::from_std(
//                 std::fs::OpenOptions::new()
//                     .append(true)
//                     .create(true)
//                     .open(log_dir.join(STDERR_FILE))?,
//             )
//         };
//         let stderr = wasi_cap_std_sync::file::File::from_cap_std(stderr);
//         // The spec does not say what to do with STDERR.
//         // See specifically sections 4.2 and 6.1 of RFC 3875.
//         // Currently, we will attach to wherever logs go.

//         let mut args = vec![uri_path.to_string()];
//         req.uri
//             .query()
//             .map(|q| q.split('&').for_each(|item| args.push(item.to_string())))
//             .take();

//         let headers: Vec<(String, String)> = headers
//             .iter()
//             .map(|(k, v)| (k.clone(), v.clone()))
//             .collect();

//         let mut builder = WasiCtxBuilder::new()
//             .args(&args)?
//             .envs(&headers)?
//             .stderr(Box::new(stderr)) // STDERR goes to the console of the server
//             .stdout(Box::new(stdout)) // STDOUT is sent to a Vec<u8>, which becomes the Body later
//             .stdin(Box::new(stdin));

//         // Map all of the volumes.
//         if let Some(dirs) = self.volumes.as_ref() {
//             for (guest, host) in dirs.iter() {
//                 debug!(%host, %guest, "Mapping volume from host to guest");
//                 // Try to open the dir or log an error.
//                 match unsafe { Dir::open_ambient_dir(host) } {
//                     Ok(dir) => {
//                         builder = builder.preopened_dir(dir, guest)?;
//                     }
//                     Err(e) => tracing::error!(%host, %guest, error = %e, "Error opening directory"),
//                 };
//             }
//         }

//         let ctx = builder.build();

//         let (mut store, engine) = self.new_store_and_engine(&global_context.cache_config_path, ctx)?;
//         let mut linker = Linker::new(&engine);
//         wasmtime_wasi::add_to_linker(&mut linker, |cx| cx)?;

//         let http = wasi_experimental_http_wasmtime::HttpCtx::new(
//             self.allowed_hosts.clone(),
//             self.http_max_concurrency,
//         )?;
//         http.add_to_linker(&mut linker)?;

//         let module = self.load_cached_module(&store, &global_context.module_cache_dir)?;
//         let instance = linker.instantiate(&mut store, &module)?;

//         // Manually drop the span so we get instantiation time
//         drop(startup_span);
//         let ep = &route_context.entrypoint;
//         // This shouldn't error out, because we already know there is a match.
//         let start = instance
//             .get_func(&mut store, ep)
//             .ok_or_else(|| anyhow::anyhow!("No such function '{}' in {}", &ep, self.module))?;

//         tracing::trace!("Calling Wasm entry point");
//         start.call(&mut store, &[])?;

//         // Okay, once we get here, all the information we need to send back in the response
//         // should be written to the STDOUT buffer. We fetch that, format it, and send
//         // it back. In the process, we might need to alter the status code of the result.
//         //
//         // This is a little janky, but basically we are looping through the output once,
//         // looking for the double-newline that distinguishes the headers from the body.
//         // The headers can then be parsed separately, while the body can be sent back
//         // to the client.
//         let out = stdout_mutex.read().unwrap();
//         let mut last = 0;
//         let mut scan_headers = true;
//         let mut buffer: Vec<u8> = Vec::new();
//         let mut out_headers: Vec<u8> = Vec::new();
//         out.iter().for_each(|i| {
//             if scan_headers && *i == 10 && last == 10 {
//                 out_headers.append(&mut buffer);
//                 buffer = Vec::new();
//                 scan_headers = false;
//                 return; // Consume the linefeed
//             }
//             last = *i;
//             buffer.push(*i)
//         });

//         let mut res = Response::new(Body::from(buffer));

//         // XXX: Does the spec allow for unknown headers to be passed to the HTTP headers?
//         let mut sufficient_response = false;
//         parse_cgi_headers(String::from_utf8(out_headers)?)
//             .iter()
//             .for_each(|h| {
//                 use hyper::header::{CONTENT_TYPE, LOCATION};
//                 match h.0.to_lowercase().as_str() {
//                     "content-type" => {
//                         sufficient_response = true;
//                         res.headers_mut().insert(CONTENT_TYPE, h.1.parse().unwrap());
//                     }
//                     "status" => {
//                         // The spec does not say that status is a sufficient response.
//                         // (It says that it may be added along with Content-Type, because
//                         // a status has a content type). However, CGI libraries in the wild
//                         // do not set content type correctly if a status is an error.
//                         // See https://datatracker.ietf.org/doc/html/rfc3875#section-6.2
//                         sufficient_response = true;
//                         // Status can be `Status CODE [STRING]`, and we just want the CODE.
//                         let status_code = h.1.split_once(' ').map(|(code, _)| code).unwrap_or(h.1);
//                         tracing::debug!(status_code, "Raw status code");
//                         match status_code.parse::<StatusCode>() {
//                             Ok(code) => *res.status_mut() = code,
//                             Err(e) => {
//                                 tracing::log::warn!("Failed to parse code: {}", e);
//                                 *res.status_mut() = StatusCode::BAD_GATEWAY;
//                             }
//                         }
//                     }
//                     "location" => {
//                         sufficient_response = true;
//                         res.headers_mut()
//                             .insert(LOCATION, HeaderValue::from_str(h.1).unwrap());
//                         *res.status_mut() = StatusCode::from_u16(302).unwrap();
//                     }
//                     _ => {
//                         // If the header can be parsed into a valid HTTP header, it is
//                         // added to the headers. Otherwise it is ignored.
//                         match HeaderName::from_lowercase(h.0.as_str().to_lowercase().as_bytes()) {
//                             Ok(hdr) => {
//                                 res.headers_mut()
//                                     .insert(hdr, HeaderValue::from_str(h.1).unwrap());
//                             }
//                             Err(e) => {
//                                 tracing::error!(error = %e, header_name = %h.0, "Invalid header name")
//                             }
//                         }
//                     }
//                 }
//             });

//         // According to the spec, a CGI script must return either a content-type
//         // or a location header. Failure to return one of these is a 500 error.
//         if !sufficient_response {
//             return Ok(internal_error(
//                 // Technically, we let `status` be sufficient, but this is more lenient
//                 // than the specification.
//                 "Exactly one of 'location' or 'content-type' must be specified",
//             ));
//         }

//         Ok(res)
//     }

//     // 
//     fn build_headers(
//         &self,
//         req: &Parts,
//         content_length: usize,
//         client_addr: SocketAddr,
//         default_host: &str,
//         use_tls: bool,
//         environment: HashMap<String, String>,
//     ) -> HashMap<String, String> {
//         let (host, port) = self.parse_host_header_uri(&req.headers, &req.uri, default_host);
//         // Note that we put these first so that there is no chance that they overwrite
//         // the built-in vars. IMPORTANT: This is also why some values have empty strings
//         // deliberately set (as opposed to omiting the pair altogether).
//         let mut headers = environment;

//         // CGI headers from RFC
//         headers.insert("AUTH_TYPE".to_owned(), "".to_owned()); // Not currently supported

//         // CONTENT_LENGTH (from the spec)
//         // The server MUST set this meta-variable if and only if the request is
//         // accompanied by a message-body entity.  The CONTENT_LENGTH value must
//         // reflect the length of the message-body after the server has removed
//         // any transfer-codings or content-codings.
//         headers.insert("CONTENT_LENGTH".to_owned(), format!("{}", content_length));

//         // CONTENT_TYPE (from the spec)
//         // The server MUST set this meta-variable if an HTTP Content-Type field is present
//         // in the client request header.  If the server receives a request with an
//         // attached entity but no Content-Type header field, it MAY attempt to determine
//         // the correct content type, otherwise it should omit this meta-variable.
//         //
//         // Right now, we don't attempt to determine a media type if none is presented.
//         //
//         // The spec seems to indicate that if CONTENT_LENGTH > 0, this may be set
//         // to "application/octet-stream" if no type is otherwise set. Not sure that is
//         // a good idea.
//         headers.insert(
//             "CONTENT_TYPE".to_owned(),
//             req.headers
//                 .get("CONTENT_TYPE")
//                 .map(|c| c.to_str().unwrap_or(""))
//                 .unwrap_or("")
//                 .to_owned(),
//         );

//         let protocol = if use_tls { "https" } else { "http" };

//         // Since this is not in the specification, an X_ is prepended, per spec.
//         // NB: It is strange that there is not a way to do this already. The Display impl
//         // seems to only provide the path.
//         let uri = req.uri.clone();
//         headers.insert(
//             "X_FULL_URL".to_owned(),
//             format!(
//                 "{}://{}:{}{}",
//                 protocol,
//                 host,
//                 port,
//                 uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("")
//             ),
//         );

//         headers.insert("GATEWAY_INTERFACE".to_owned(), WAGI_VERSION.to_owned());

//         // This is the Wagi route. This is different from PATH_INFO in that it may
//         // have a trailing '/...'
//         headers.insert("X_MATCHED_ROUTE".to_owned(), self.route.to_owned());

//         headers.insert(
//             "QUERY_STRING".to_owned(),
//             req.uri.query().unwrap_or("").to_owned(),
//         );

//         headers.insert("REMOTE_ADDR".to_owned(), client_addr.ip().to_string());
//         headers.insert("REMOTE_HOST".to_owned(), client_addr.ip().to_string()); // The server MAY substitute it with REMOTE_ADDR
//         headers.insert("REMOTE_USER".to_owned(), "".to_owned()); // TODO: Parse this out of uri.authority?
//         headers.insert("REQUEST_METHOD".to_owned(), req.method.to_string());

//         // The Path component is /$SCRIPT_NAME/$PATH_INFO
//         // SCRIPT_NAME is the route that matched.
//         // https://datatracker.ietf.org/doc/html/rfc3875#section-4.1.13
//         let script_name = self
//             .route
//             .strip_suffix("/...")
//             .map(|i| {
//                 if i.starts_with('/') {
//                     i.to_owned()
//                 } else {
//                     format!("/{}", i)
//                 }
//             }) // At the bare minimum, SCRIPT_NAME must be '/'
//             .unwrap_or_else(|| self.route.clone());
//         headers.insert("SCRIPT_NAME".to_owned(), script_name);
//         // PATH_INFO is any path information after SCRIPT_NAME
//         //
//         // I am intentionally ignoring the PATH_INFO rule that says that a PATH_INFO
//         // cannot have a path seperator in it. If it becomes important to distinguish
//         // between what was decoded out of the path and what is encoded in the path,
//         // the X_RAW_PATH_INFO can be used.
//         //
//         // https://datatracker.ietf.org/doc/html/rfc3875#section-4.1.5
//         let pathsegment = self.path_info(req.uri.path());
//         let pathinfo = url_escape::decode(&pathsegment);
//         headers.insert("X_RAW_PATH_INFO".to_owned(), pathsegment.clone());
//         headers.insert("PATH_INFO".to_owned(), pathinfo.to_string());
//         // PATH_TRANSLATED is the url-decoded version of PATH_INFO
//         // https://datatracker.ietf.org/doc/html/rfc3875#section-4.1.6
//         headers.insert("PATH_TRANSLATED".to_owned(), pathinfo.to_string());

//         // From the spec: "the server would use the contents of the request's Host header
//         // field to select the correct virtual host."
//         headers.insert("SERVER_NAME".to_owned(), host);
//         headers.insert("SERVER_PORT".to_owned(), port);
//         headers.insert("SERVER_PROTOCOL".to_owned(), format!("{:?}", req.version));

//         headers.insert(
//             "SERVER_SOFTWARE".to_owned(),
//             SERVER_SOFTWARE_VERSION.to_owned(),
//         );

//         // Normalize incoming HTTP headers. The spec says:
//         // "The HTTP header field name is converted to upper case, has all
//         // occurrences of "-" replaced with "_" and has "HTTP_" prepended to
//         // give the meta-variable name."
//         req.headers.iter().for_each(|header| {
//             let key = format!(
//                 "HTTP_{}",
//                 header.0.as_str().to_uppercase().replace("-", "_")
//             );
//             // Per spec 4.1.18, skip some headers
//             if key == "HTTP_AUTHORIZATION" || key == "HTTP_CONNECTION" {
//                 return;
//             }
//             let val = header.1.to_str().unwrap_or("CORRUPT VALUE").to_owned();
//             headers.insert(key, val);
//         });

//         headers
//     }

// }

// This is currently separated out because it has different ownership
// constraints from the stdout_mutex. Not sure how to do this better.
// (I don't want to .clone() the fields even though that would work,
// because that is misleading about the semantics.)
pub struct IOStreamRedirects {
    pub stdin: ReadPipe<std::io::Cursor<Vec<u8>>>,
    pub stdout: WritePipe<Vec<u8>>,
    pub stderr: wasi_cap_std_sync::file::File,
}

pub struct IORedirectionInfo {
    pub streams: IOStreamRedirects,
    pub stdout_mutex: Arc<RwLock<Vec<u8>>>,
}
