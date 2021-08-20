//! The tools for executing WAGI modules, and managing the lifecycle of a request.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::{collections::HashMap, net::SocketAddr};
use std::{
    hash::{Hash, Hasher},
    io::BufRead,
};

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

use crate::version::*;
use crate::{http_util::*, runtime::bindle::bindle_cache_key};

pub mod bindle;

/// The default Bindle server URL.
pub const DEFAULT_BINDLE_SERVER: &str = "http://localhost:8080/v1";

const WASM_LAYER_CONTENT_TYPE: &str = "application/vnd.wasm.content.layer.v1+wasm";
const STDERR_FILE: &str = "module.stderr";

pub struct RouterInfo {
    pub entrypoint: String,
    pub client_addr: SocketAddr,
    pub cache_config_path: PathBuf,
    pub module_cache_dir: PathBuf,
    pub base_log_dir: PathBuf,
    pub default_host: String,
    pub use_tls: bool,
    pub env_vars: HashMap<String, String>,
}

/// An internal representation of a mapping from a URI fragment to a function in a module.
#[derive(Clone)]
pub struct RouteEntry {
    pub path: String,
    pub entrypoint: String,
}

/// A handler contains all of the information necessary to execute the correct function on a module.
#[derive(Clone, Debug)]
pub struct Handler {
    /// A reference to the module for this handler.
    pub module: Module,
    /// The function that should be called to handle this path.
    pub entrypoint: String,
    /// The path pattern that this handler answers.
    ///
    // For example, an exact path `/foo/bar` may be returned, as may a wildcard path such as `/foo/...`
    //
    // This path is the _fully constructed_ path. That is, if a module config declares its path as `/base`,
    // and the module registers the path `/foo/...`, the value of this would be `/base/foo/...`.
    pub path: String,
}

impl Handler {
    /// Given a module and a route entry, create a new handler.
    pub fn new(entry: RouteEntry, module: Module) -> Self {
        Handler {
            path: entry.path,
            entrypoint: entry.entrypoint,
            module,
        }
    }
}

/// Description of a single WAGI module
#[derive(Clone, Debug, Deserialize)]
pub struct Module {
    /// The route, begining with a leading slash.
    ///
    /// The suffix "/..." means "this route and all sub-paths". For example, the route
    /// "/foo/..." will match "/foo" as well as "/foo/bar" and "/foo/bar/baz"
    pub route: String,
    /// The path to the module that will be loaded.
    ///
    /// This should be an absolute path. It must point to a WASM+WASI 32-bit program
    /// with the read bit set.
    pub module: String,
    /// Directories on the local filesystem that can be opened by this module
    /// The key (left value) is the name of the directory INSIDE the WASM. The value is
    /// the location OUTSIDE the WASM. Two inside locations can map to the same outside
    /// location.
    pub volumes: Option<HashMap<String, String>>,
    /// The name of the function that is the entrypoint for executing the module.
    /// The default is `_start`.
    pub entrypoint: Option<String>,
    /// The URL fragment for the bindle server.
    ///
    /// If none is supplied, then http://localhost:8080/v1 is used
    pub bindle_server: Option<String>,

    /// List of hosts that the guest module is allowed to make HTTP requests to.
    /// If none or an empty vector is supplied, the guest module cannot send
    /// requests to any server.
    pub allowed_hosts: Option<Vec<String>>,

    /// Max http concurrency that the guest module configures for the HTTP
    /// client. If none, the guest module uses the default concurrency provided
    /// by the WASM HTTP client module.
    pub http_max_concurrency: Option<u32>,
}

// For hashing, we don't need all of the fields to hash. A wasm module (not a `Module`) can be used
// multiple times and configured different ways, but the route can only be used once per WAGI
// instance
impl Hash for Module {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.route.hash(state);
    }
}

impl PartialEq for Module {
    fn eq(&self, other: &Self) -> bool {
        self.route == other.route
    }
}

impl Eq for Module {}

impl Module {
    pub fn new(route: String, module_uri: String) -> Self {
        Module {
            route,
            module: module_uri,
            volumes: None,
            entrypoint: None,
            allowed_hosts: None,
            bindle_server: None,
            http_max_concurrency: None,
        }
    }

    /// Execute the WASM module in a WAGI
    ///
    /// The given `base_log_dir` should be a directory where all module logs will be stored. When
    /// executing a module, a subdirectory will be created in this directory with the ID (from the
    /// [`id` method](Module::id)) for its name. The log will be placed in that directory at
    /// `module.stderr`
    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "trace", skip(self, req, info), fields(route = %self.route, module = %self.module))]
    pub async fn execute(&self, req: Request<Body>, info: RouterInfo) -> Response<Body> {
        // Read the parts in here
        let (parts, body) = req.into_parts();
        let data = hyper::body::to_bytes(body)
            .await
            .unwrap_or_default()
            .to_vec();
        let me = self.clone();
        let res = match tokio::task::spawn_blocking(move || me.run_wasm(&parts, data, info)).await {
            Ok(res) => res,
            Err(e) if e.is_panic() => {
                tracing::error!(error = %e, "Recoverable panic on Wasm Runner thread");
                return internal_error("Module run error");
            }
            Err(e) => {
                tracing::error!(error = %e, "Recoverable panic on Wasm Runner thread");
                return internal_error("module run was cancelled");
            }
        };
        match res {
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

    /// Returns the unique ID of the module.
    ///
    /// This is the SHA256 sum of the following data, written into the hasher in the following order
    /// (skipping any `None`s):
    ///
    /// - route
    /// - host
    pub fn id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(&self.route);
        format!("{:x}", hasher.finalize())
    }

    /// Examine the given module to see if it has any routes.
    ///
    /// If it has any routes, add them to the vector and return it. The given `base_log_dir` should
    /// be a directory where all module logs will be stored. When executing a module, a subdirectory
    /// will be created in this directory with the ID (from the [`id` method](Module::id)) for its
    /// name. The log will be placed in that directory at `module.stderr`
    #[instrument(
        level = "trace",
        skip(self, cache_config_path, module_cache_dir, base_log_dir)
    )]
    pub(crate) fn load_routes(
        &self,
        cache_config_path: &Path,
        module_cache_dir: &Path,
        base_log_dir: &Path,
    ) -> Result<Vec<RouteEntry>, anyhow::Error> {
        let startup_span = tracing::info_span!("route_instantiation").entered();

        let prefix = self
            .route
            .strip_suffix("/...")
            .unwrap_or_else(|| self.route.as_str());
        let mut routes = vec![RouteEntry {
            path: self.route.to_owned(), // We don't use prefix because prefix has been normalized.
            entrypoint: self
                .entrypoint
                .clone()
                .unwrap_or_else(|| "_start".to_string()),
        }];

        // TODO: We should dedup this code somewhere because there are plenty of similarities to
        // `run_wasm`

        // Make sure the directory exists
        let log_dir = base_log_dir.join(self.id());
        std::fs::create_dir_all(&log_dir)?;
        // Open a file for appending. Right now this will just keep appending as there is no log
        // rotation or cleanup
        let stderr = unsafe {
            cap_std::fs::File::from_std(
                std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(log_dir.join(STDERR_FILE))?,
            )
        };
        let stderr = wasi_cap_std_sync::file::File::from_cap_std(stderr);

        let stdout_buf: Vec<u8> = vec![];
        let stdout_mutex = Arc::new(RwLock::new(stdout_buf));
        let stdout = WritePipe::from_shared(stdout_mutex.clone());

        let ctx = WasiCtxBuilder::new()
            .stderr(Box::new(stderr))
            .stdout(Box::new(stdout))
            .build();

        let (mut store, engine) = self.new_store_and_engine(cache_config_path, ctx)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::add_to_linker(&mut linker, |cx| cx)?;

        let http = wasi_experimental_http_wasmtime::HttpCtx::new(None, None)?;
        http.add_to_linker(&mut linker)?;

        let module = self.load_cached_module(&store, module_cache_dir)?;
        let instance = linker.instantiate(&mut store, &module)?;

        // Manually drop the span to get the instantiation time
        drop(startup_span);

        match instance.get_func(&mut store, "_routes") {
            Some(func) => {
                func.call(&mut store, &[])?;
            }
            None => return Ok(routes),
        }

        let out = stdout_mutex.read().unwrap();
        out.lines().for_each(|line_result| {
            if let Ok(line) = line_result {
                // Split line into parts
                let parts: Vec<&str> = line.trim().split_whitespace().collect();

                if parts.is_empty() {
                    return;
                }

                let key = parts.get(0).unwrap_or(&"/").to_string();
                let val = parts.get(1).unwrap_or(&"_start").to_string();
                routes.push(RouteEntry {
                    path: format!("{}{}", prefix, key),
                    entrypoint: val,
                });
            }
        });
        // We reverse the routes so that the top-level routes are evaluated last.
        // This gives a predictable order for traversing routes. Because the base path
        // is the last one evaluated, if the base path is /..., it will match when no
        // other more specific route lasts.
        //
        // Additionally, when Wasm authors create their _routes() callback, they can
        // organize their outputs to match according to their own precedence merely by
        // putting the higher precedence routes at the end of the output.
        routes.reverse();
        Ok(routes)
    }

    /// Build the WAGI headers for injection into a module.
    fn build_headers(
        &self,
        req: &Parts,
        content_length: usize,
        client_addr: SocketAddr,
        default_host: &str,
        use_tls: bool,
        environment: HashMap<String, String>,
    ) -> HashMap<String, String> {
        let (host, port) = self.parse_host_header_uri(&req.headers, &req.uri, default_host);
        // Note that we put these first so that there is no chance that they overwrite
        // the built-in vars. IMPORTANT: This is also why some values have empty strings
        // deliberately set (as opposed to omiting the pair altogether).
        let mut headers = environment;

        // CGI headers from RFC
        headers.insert("AUTH_TYPE".to_owned(), "".to_owned()); // Not currently supported

        // CONTENT_LENGTH (from the spec)
        // The server MUST set this meta-variable if and only if the request is
        // accompanied by a message-body entity.  The CONTENT_LENGTH value must
        // reflect the length of the message-body after the server has removed
        // any transfer-codings or content-codings.
        headers.insert("CONTENT_LENGTH".to_owned(), format!("{}", content_length));

        // CONTENT_TYPE (from the spec)
        // The server MUST set this meta-variable if an HTTP Content-Type field is present
        // in the client request header.  If the server receives a request with an
        // attached entity but no Content-Type header field, it MAY attempt to determine
        // the correct content type, otherwise it should omit this meta-variable.
        //
        // Right now, we don't attempt to determine a media type if none is presented.
        //
        // The spec seems to indicate that if CONTENT_LENGTH > 0, this may be set
        // to "application/octet-stream" if no type is otherwise set. Not sure that is
        // a good idea.
        headers.insert(
            "CONTENT_TYPE".to_owned(),
            req.headers
                .get("CONTENT_TYPE")
                .map(|c| c.to_str().unwrap_or(""))
                .unwrap_or("")
                .to_owned(),
        );

        let protocol = if use_tls { "https" } else { "http" };

        // Since this is not in the specification, an X_ is prepended, per spec.
        // NB: It is strange that there is not a way to do this already. The Display impl
        // seems to only provide the path.
        let uri = req.uri.clone();
        headers.insert(
            "X_FULL_URL".to_owned(),
            format!(
                "{}://{}:{}{}",
                protocol,
                host,
                port,
                uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("")
            ),
        );

        headers.insert("GATEWAY_INTERFACE".to_owned(), WAGI_VERSION.to_owned());
        headers.insert("X_MATCHED_ROUTE".to_owned(), self.route.to_owned()); // Specific to WAGI (not CGI)
        headers.insert("PATH_INFO".to_owned(), req.uri.path().to_owned()); // TODO: Does this get trimmed?

        // This also does not appear in the specification for CGI (largely because CGI did
        // not necessarily "know about" URL rewrites). But it is very useful when combined
        // with wildcard pattern matching.
        headers.insert(
            "X_RELATIVE_PATH".to_owned(),
            self.x_relative_path(req.uri.path()),
        );

        // NOTE: The security model of WAGI means that we do not give the actual
        // translated path on the host filesystem, as that is off limits to the runtime.
        // Right now, this just returns the same as PATH_INFO, but we could attempt to
        // map it to something if we know what that "something" is.
        headers.insert("PATH_TRANSLATED".to_owned(), req.uri.path().to_owned());
        headers.insert(
            "QUERY_STRING".to_owned(),
            req.uri.query().unwrap_or("").to_owned(),
        );

        headers.insert("REMOTE_ADDR".to_owned(), client_addr.ip().to_string());
        headers.insert("REMOTE_HOST".to_owned(), client_addr.ip().to_string()); // The server MAY substitute it with REMOTE_ADDR
        headers.insert("REMOTE_USER".to_owned(), "".to_owned()); // TODO: Parse this out of uri.authority?
        headers.insert("REQUEST_METHOD".to_owned(), req.method.to_string());
        headers.insert("SCRIPT_NAME".to_owned(), self.module.to_owned());
        // From the spec: "the server would use the contents of the request's Host header
        // field to select the correct virtual host."
        headers.insert("SERVER_NAME".to_owned(), host);
        headers.insert("SERVER_PORT".to_owned(), port);
        headers.insert("SERVER_PROTOCOL".to_owned(), format!("{:?}", req.version));

        headers.insert(
            "SERVER_SOFTWARE".to_owned(),
            SERVER_SOFTWARE_VERSION.to_owned(),
        );

        // Normalize incoming HTTP headers. The spec says:
        // "The HTTP header field name is converted to upper case, has all
        // occurrences of "-" replaced with "_" and has "HTTP_" prepended to
        // give the meta-variable name."
        req.headers.iter().for_each(|header| {
            let key = format!(
                "HTTP_{}",
                header.0.as_str().to_uppercase().replace("-", "_")
            );
            // Per spec 4.1.18, skip some headers
            if key == "HTTP_AUTHORIZATION" || key == "HTTP_CONNECTION" {
                return;
            }
            let val = header.1.to_str().unwrap_or("CORRUPT VALUE").to_owned();
            headers.insert(key, val);
        });

        headers
    }

    /// Internal utility function for parsing a host header.
    ///
    /// This attempts to use three sources to construct a definitive host/port pair, ordering
    /// by precedent.
    ///
    /// - The content of the host header is considered most authoritative.
    /// - Next most authoritative is self.host, which is set at the CLI or in the config
    /// - As a last resort, we use the host/port that Hyper gives us.
    /// - If none of these provide sufficient data, which is definitely a possiblity,
    ///   we go with `localhost` as host and `80` as port. This, of course, is problematic,
    ///   but should only manifest if both the server and the client are behaving badly.
    fn parse_host_header_uri(
        &self,
        headers: &HeaderMap,
        uri: &hyper::Uri,
        default_host: &str,
    ) -> (String, String) {
        let host_header = headers.get(HOST).and_then(|v| match v.to_str() {
            Err(_) => None,
            Ok(s) => Some(s.to_owned()),
        });

        let mut host = uri
            .host()
            .map(|h| h.to_string())
            .unwrap_or_else(|| "localhost".to_owned());
        let mut port = uri.port_u16().unwrap_or(80).to_string();

        let mut parse_host = |hdr: String| {
            let mut parts = hdr.splitn(2, ':');
            match parts.next() {
                Some(h) if !h.is_empty() => host = h.to_owned(),
                _ => {}
            }
            match parts.next() {
                Some(p) if !p.is_empty() => {
                    debug!(port = p, "Overriding port");
                    port = p.to_owned()
                }
                _ => {}
            }
        };

        // Override with local host field if set.
        if !default_host.is_empty() {
            parse_host(default_host.to_owned());
        }

        // Finally, the value of the HOST header is considered authoritative.
        // When it comes to port number, the HOST header isn't necessarily 100% trustworthy.
        // But it appears that this is still the best behavior for the CGI spec.
        if let Some(hdr) = host_header {
            parse_host(hdr);
        }

        (host, port)
    }

    /// Resolve a relative path from the end of the matched path to the end of the string.
    ///
    /// For example, if the match is `/foo/...` and the path is `/foo/bar`, it should return `"bar"`,
    /// but if the match is `/foo/bar` and the path is `/foo/bar`, it should return `""`.
    fn x_relative_path(&self, uri_path: &str) -> String {
        uri_path
            .strip_prefix(
                // Chop the `/...` off of the end if there is one.
                self.route
                    .strip_suffix("/...")
                    .unwrap_or_else(|| self.route.as_str()),
            )
            // Remove a leading `/` if there is one.
            .map(|r| r.strip_prefix("/").unwrap_or(r))
            // It is possible that a root path request matching /... returns a None here,
            // so in that case the appropriate return is "".
            .unwrap_or("")
            .to_owned()
    }

    // Load and execute the WASM module.
    //
    // Typically, the higher-level execute() method should be used instead, as that handles
    // wrapping errors in the appropriate HTTP response. This is a lower-level function
    // that returns the errors that occur during processing of a WASM module.
    //
    // Note that on occasion, this module COULD return an Ok() with a response body that
    // contains an HTTP error. This can occur, for example, if the WASM module sets
    // the status code on its own.
    //

    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "info", skip(self, req, body, info), fields(uri = %req.uri, module = %self.module, use_tls = %info.use_tls, env = ?info.env_vars))]
    fn run_wasm(
        &self,
        req: &Parts,
        body: Vec<u8>,
        info: RouterInfo,
    ) -> Result<Response<Body>, anyhow::Error> {
        let startup_span = tracing::info_span!("module instantiation").entered();
        let uri_path = req.uri.path();
        let headers = self.build_headers(
            req,
            body.len(),
            info.client_addr,
            info.default_host.as_str(),
            info.use_tls,
            info.env_vars,
        );
        let stdin = ReadPipe::from(body);
        let stdout_buf: Vec<u8> = vec![];
        let stdout_mutex = Arc::new(RwLock::new(stdout_buf));
        let stdout = WritePipe::from_shared(stdout_mutex.clone());

        // Make sure the directory exists
        let log_dir = info.base_log_dir.join(self.id());
        tracing::info!(log_dir = %log_dir.display(), "Using log dir");
        std::fs::create_dir_all(&log_dir)?;
        // Open a file for appending. Right now this will just keep appending as there is no log
        // rotation or cleanup
        let stderr = unsafe {
            cap_std::fs::File::from_std(
                std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(log_dir.join(STDERR_FILE))?,
            )
        };
        let stderr = wasi_cap_std_sync::file::File::from_cap_std(stderr);
        // The spec does not say what to do with STDERR.
        // See specifically sections 4.2 and 6.1 of RFC 3875.
        // Currently, we will attach to wherever logs go.

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
            .stderr(Box::new(stderr)) // STDERR goes to the console of the server
            .stdout(Box::new(stdout)) // STDOUT is sent to a Vec<u8>, which becomes the Body later
            .stdin(Box::new(stdin));

        // Map all of the volumes.
        if let Some(dirs) = self.volumes.as_ref() {
            for (guest, host) in dirs.iter() {
                debug!(%host, %guest, "Mapping volume from host to guest");
                // Try to open the dir or log an error.
                match unsafe { Dir::open_ambient_dir(host) } {
                    Ok(dir) => {
                        builder = builder.preopened_dir(dir, guest)?;
                    }
                    Err(e) => tracing::error!(%host, %guest, error = %e, "Error opening directory"),
                };
            }
        }

        let ctx = builder.build();

        let (mut store, engine) = self.new_store_and_engine(&info.cache_config_path, ctx)?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::add_to_linker(&mut linker, |cx| cx)?;

        let http = wasi_experimental_http_wasmtime::HttpCtx::new(
            self.allowed_hosts.clone(),
            self.http_max_concurrency.clone(),
        )?;
        http.add_to_linker(&mut linker)?;

        let module = self.load_cached_module(&store, &info.module_cache_dir)?;
        let instance = linker.instantiate(&mut store, &module)?;

        // Manually drop the span so we get instantiation time
        drop(startup_span);
        let ep = &info.entrypoint;
        // This shouldn't error out, because we already know there is a match.
        let start = instance
            .get_func(&mut store, ep)
            .ok_or_else(|| anyhow::anyhow!("No such function '{}' in {}", &ep, self.module))?;

        tracing::trace!("Calling Wasm entry point");
        start.call(&mut store, &[])?;

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

        // XXX: Does the spec allow for unknown headers to be passed to the HTTP headers?
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
                        if let Ok(status) = h.1.parse::<hyper::StatusCode>() {
                            tracing::info!(%status, "Setting status");
                            *res.status_mut() = status;
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

        // According to the spec, a CGI script must return either a content-type
        // or a location header. Failure to return one of these is a 500 error.
        if !sufficient_response {
            return Ok(internal_error(
                "Exactly one of 'location' or 'content-type' must be specified",
            ));
        }

        Ok(res)
    }

    /// Determine the source of the module, and read it from that source.
    ///
    /// Modules can be stored locally, or they can be stored in external sources like
    /// Bindle. WAGI determines the source by looking at the URI of the module.
    ///
    /// - If `file:` is specified, or no schema is specified, this loads from the local filesystem
    /// - If `bindle:` is specified, this will retrieve the module from the configured Bindle server
    /// - If `oci:` is specified, this will retrieve the module from an OCI Distribution registry
    ///
    /// While `file` is a little lenient in its adherence to the URL spec, `bindle` and `oci` are not.
    /// For example, an `oci` URL that references `alpine:latest` should be `oci:alpine:latest`.
    /// It should NOT be `oci://alpine:latest` because `alpine` is not a host name.
    async fn load_module(
        &self,
        store: &Store<WasiCtx>,
        cache: &Path,
    ) -> anyhow::Result<wasmtime::Module> {
        tracing::trace!(
            module = %self.module,
            "Loading from source"
        );
        match Url::parse(self.module.as_str()) {
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "Error parsing module URI. Assuming this is a local file"
                );
                wasmtime::Module::from_file(store.engine(), self.module.as_str())
            }
            Ok(uri) => match uri.scheme() {
                "file" => {
                    match uri.to_file_path() {
                        Ok(p) => return wasmtime::Module::from_file(store.engine(), p),
                        Err(e) => anyhow::bail!("Cannot get path to file: {:#?}", e),
                    };
                }
                "bindle" => self.load_bindle(&uri, store.engine(), cache).await,
                "parcel" => self.load_parcel(&uri, store.engine(), cache).await,
                "oci" => self.load_oci(&uri, store.engine(), cache).await,
                s => anyhow::bail!("Unknown scheme {}", s),
            },
        }
    }

    /// Load a cached module from the filesystem.
    ///
    /// This is synchronous right now because Wasmtime on the runner needs to be run synchronously.
    /// This will change when the new version of Wasmtime adds Send + Sync to all the things.
    /// Then we can just do `load_module` or refactor this to be async.
    #[instrument(level = "info", skip(self, store, cache_dir), fields(cache = %cache_dir.display(), module = %self.module))]
    fn load_cached_module(
        &self,
        store: &Store<WasiCtx>,
        cache_dir: &Path,
    ) -> anyhow::Result<wasmtime::Module> {
        let canonical_path = match Url::parse(self.module.as_str()) {
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "Error parsing module URI. Assuming this is a local file."
                );
                PathBuf::from(self.module.as_str())
            }
            Ok(uri) => match uri.scheme() {
                "file" => match uri.to_file_path() {
                    Ok(p) => p,
                    Err(e) => anyhow::bail!("Cannot get path to file: {:#?}", e),
                },
                "bindle" => cache_dir.join(bindle_cache_key(&uri)),
                "parcel" => {
                    // parcel: bindle_uri#SHA256 becomes cache_dir/SHA256
                    let cache_file = uri.fragment().unwrap_or_else(|| uri.path()); // should always have fragment
                    cache_dir.join(cache_file)
                }
                "oci" => cache_dir.join(self.hash_name()),
                s => {
                    tracing::error!(scheme = s, "unknown scheme in module");
                    anyhow::bail!("Unknown scheme {}", s)
                }
            },
        };
        tracing::trace!(?canonical_path);

        // If there is a module at this path, load it.
        // Right now, _any_ problem loading the module will result in us trying to
        // re-fetch it.
        match wasmtime::Module::from_file(store.engine(), canonical_path) {
            Ok(module) => Ok(module),
            Err(_e) => {
                tracing::debug!("module cache miss. Loading module from remote.");
                // TODO: This could be reallllllllly dangerous as we are for sure going to block at this
                // point on this current thread. This _should_ be ok given that we run this as a
                // spawn_blocking, but those sound like famous last words waiting to happen. Refactor this
                // sooner rather than later
                futures::executor::block_on(self.load_module(&store, cache_dir))
            }
        }
    }

    fn hash_name(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(&self.module.as_str());
        let result = hasher.finalize();
        format!("{:x}", result)
    }

    #[instrument(level = "info", skip(self, engine, cache), fields(server = ?self.bindle_server))]
    async fn load_bindle(
        &self,
        uri: &Url,
        engine: &Engine,
        cache: &Path,
    ) -> anyhow::Result<wasmtime::Module> {
        bindle::load_bindle(
            self.bindle_server
                .clone()
                .unwrap_or_else(|| DEFAULT_BINDLE_SERVER.to_owned())
                .as_str(),
            uri,
            engine,
            cache,
        )
        .await
    }

    #[instrument(level = "info", skip(self, engine, cache))]
    async fn load_parcel(
        &self,
        uri: &Url,
        engine: &Engine,
        cache: &Path,
    ) -> anyhow::Result<wasmtime::Module> {
        let bs = self
            .bindle_server
            .clone()
            .unwrap_or_else(|| DEFAULT_BINDLE_SERVER.to_owned());
        bindle::load_parcel(bs.as_str(), uri, engine, cache).await
    }

    #[instrument(level = "info", skip(self, engine, cache))]
    async fn load_oci(
        &self,
        uri: &Url,
        engine: &Engine,
        cache: &Path,
    ) -> anyhow::Result<wasmtime::Module> {
        let config = ClientConfig {
            protocol: oci_distribution::client::ClientProtocol::HttpsExcept(vec![
                "localhost:5000".to_owned(),
                "127.0.0.1:5000".to_owned(),
            ]),
        };
        let mut oc = Client::new(config);
        let auth = RegistryAuth::Anonymous;

        let img = url_to_oci(uri).map_err(|e| {
            tracing::error!(
                error = %e,
                "Could not convert uri to OCI reference"
            );
            e
        })?;
        let data = oc
            .pull(&img, &auth, vec![WASM_LAYER_CONTENT_TYPE])
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Pull failed");
                e
            })?;
        if data.layers.is_empty() {
            tracing::error!(image = %img, "Image has no layers");
            anyhow::bail!("image has no layers");
        }
        let first_layer = data.layers.get(0).unwrap();

        // If a cache write fails, log it but continue on.
        tracing::trace!("writing layer to module cache");
        if let Err(e) =
            tokio::fs::write(cache.join(self.hash_name()), first_layer.data.as_slice()).await
        {
            tracing::warn!(error = %e, "failed to write module to cache");
        }
        let module = wasmtime::Module::new(engine, first_layer.data.as_slice())?;
        Ok(module)
    }

    fn new_store_and_engine(
        &self,
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

/// Build the image name from the URL passed in.
/// So oci://example.com/foo:latest will become example.com/foo:latest
///
/// If parsing fails, this will emit an error.
fn url_to_oci(uri: &Url) -> anyhow::Result<Reference> {
    let name = uri.path().trim_start_matches('/');
    let port = uri.port().map(|p| format!(":{}", p)).unwrap_or_default();
    let r: Reference = match uri.host() {
        Some(host) => format!("{}{}/{}", host, port, name).parse(),
        None => name.parse(),
    }?;
    Ok(r) // Because who doesn't love OKRs.
}

#[cfg(test)]
mod test {
    use super::{url_to_oci, Module};
    use crate::ModuleConfig;

    use hyper::http::request::Request;
    use std::io::Write;
    use std::path::PathBuf;
    use std::str::FromStr;
    use tempfile::NamedTempFile;
    use wasi_cap_std_sync::WasiCtxBuilder;
    use wasmtime::Engine;
    use wasmtime::Store;

    const ROUTES_WAT: &str = r#"
    (module
        (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
        (memory 1)
        (export "memory" (memory 0))

        (data (i32.const 8) "/one one\n/two/... two\n")

        (func $main (export "_routes")
            (i32.store (i32.const 0) (i32.const 8))
            (i32.store (i32.const 4) (i32.const 22))

            (call $fd_write
                (i32.const 1)
                (i32.const 0)
                (i32.const 1)
                (i32.const 20)
            )
            drop
        )
    )
    "#;

    fn write_temp_wat(data: &str) -> anyhow::Result<NamedTempFile> {
        let mut tf = tempfile::NamedTempFile::new()?;
        write!(tf, "{}", data)?;
        Ok(tf)
    }

    #[tokio::test]
    async fn load_routes_from_wasm() {
        let tf = write_temp_wat(ROUTES_WAT).expect("created tempfile");
        let urlish = format!("file:{}", tf.path().to_string_lossy());

        let cache = PathBuf::from("cache.toml");

        // We should be able to mount the same wasm at a separate route.
        let module = Module::new("/base".to_string(), urlish.clone());
        let module2 = Module::new("/another/...".to_string(), urlish);

        let mut mc = ModuleConfig {
            modules: vec![module.clone(), module2.clone()].into_iter().collect(),
            route_cache: None,
        };

        let log_tempdir = tempfile::tempdir().expect("Unable to create tempdir");
        let cache_tempdir = tempfile::tempdir().expect("new cache temp dir");
        mc.build_registry(&cache, cache_tempdir.path(), log_tempdir.path())
            .await
            .expect("registry build cleanly");

        tracing::debug!(route_cache = ?mc.route_cache);

        // Three routes for each module.
        assert_eq!(6, mc.route_cache.as_ref().expect("routes are set").len());

        let modpath = module.module.clone();

        // Base route is from the config file
        let base = mc
            .handler_for_path("/base")
            .expect("Should get a /base route");
        assert_eq!("_start", base.entrypoint);
        assert_eq!(modpath, base.module.module);

        // Route one is from the module's _routes()
        let one = mc
            .handler_for_path("/base/one")
            .expect("Should get the /base/one route");

        assert_eq!("one", one.entrypoint);
        assert_eq!(modpath, one.module.module);

        // Route two is a wildcard.
        let two = mc
            .handler_for_path("/base/two/three")
            .expect("Should get the /base/two/... route");

        assert_eq!("two", two.entrypoint);
        assert_eq!(modpath, two.module.module);

        // This should fail
        assert!(mc.handler_for_path("/base/no/such/path").is_err());

        // This should pass
        mc.handler_for_path("/another/path")
            .expect("The generic handler should have been returned for this");
    }

    #[test]
    fn should_produce_relative_path() {
        let uri_path = "/static/images/icon.png";
        let mut m = Module::new("/static/...".to_owned(), "/tmp/fake".to_owned());
        assert_eq!("images/icon.png", m.x_relative_path(uri_path));

        m.route = "/static/images/icon.png".to_owned();
        assert_eq!("", m.x_relative_path(uri_path));

        m.route = "/...".to_owned();
        assert_eq!("", m.x_relative_path("/"));

        m.route = "/".to_owned();
        assert_eq!("", m.x_relative_path("/"));

        // As a degenerate case, if the path does not match the prefix,
        // then it should return an empty path because this is not
        // a relative path from the given path. While this is a no-op in
        // current Wagi, conceivably we could some day have to alter this
        // behavior. So this test is a canary for a breaking change.
        m.route = "/foo".to_owned();
        assert_eq!("", m.x_relative_path("/bar"));
    }

    #[tokio::test]
    async fn should_parse_file_uri() {
        let tf = write_temp_wat(ROUTES_WAT).expect("wrote tempfile");
        let urlish = format!("file:{}", tf.path().to_string_lossy());

        let module = Module::new("/base".to_string(), urlish);

        let ctx = WasiCtxBuilder::new().build();
        let engine = Engine::default();
        let store = Store::new(&engine, ctx);
        let tempdir = tempfile::tempdir().expect("create a temp dir");

        module
            .load_module(&store, tempdir.path())
            .await
            .expect("loaded module");
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn should_parse_file_with_all_the_windows_slashes() {
        let tf = write_temp_wat(ROUTES_WAT).expect("wrote tempfile");
        let testcases = possible_slashes_for_paths(tf.path().to_string_lossy().to_string());
        for test in testcases {
            let module = Module::new("/base".to_string(), test);
            let ctx = WasiCtxBuilder::new().build();
            let engine = Engine::default();
            let store = Store::new(&engine, ctx);
            let tempdir = tempfile::tempdir().expect("create a temp dir");

            module
                .load_module(&store, tempdir.path())
                .await
                .expect("loaded module");
        }
    }

    #[cfg(target_os = "windows")]
    fn possible_slashes_for_paths(path: String) -> Vec<String> {
        let mut res = vec![];

        // this should transform the initial Windows path coming from
        // the temoporary file to most common ways to define a module
        // in modules.toml.

        res.push(format!("file:{}", path));
        res.push(format!("file:/{}", path));
        res.push(format!("file://{}", path));
        res.push(format!("file:///{}", path));

        let double_backslash = str::replace(path.as_str(), "\\", "\\\\");
        res.push(format!("file:{}", double_backslash));
        res.push(format!("file:/{}", double_backslash));
        res.push(format!("file://{}", double_backslash));
        res.push(format!("file:///{}", double_backslash));

        let forward_slash = str::replace(path.as_str(), "\\", "/");
        res.push(format!("file:{}", forward_slash));
        res.push(format!("file:/{}", forward_slash));
        res.push(format!("file://{}", forward_slash));
        res.push(format!("file:///{}", forward_slash));

        let double_slash = str::replace(path.as_str(), "\\", "//");
        res.push(format!("file:{}", double_slash));
        res.push(format!("file:/{}", double_slash));
        res.push(format!("file://{}", double_slash));
        res.push(format!("file:///{}", double_slash));

        res
    }

    // Why is there a test for upstream libraries? Well, because they each seem to have
    // quirks that cause them to differ from the spec. This is here because we plan on
    // changing to Hyper when it gets updated, but for now are using URL.
    //
    // Note that `url` follows the WhatWG convention of omitting `localhost` in `file:` urls.
    #[test]
    fn should_parse_file_scheme() {
        let uri = url::Url::parse("file:///foo/bar").expect("Should parse URI with no host");
        assert!(uri.host().is_none());

        let uri = url::Url::parse("file:/foo/bar").expect("Should parse URI with no host");
        assert!(uri.host().is_none());

        let uri =
            url::Url::parse("file://localhost/foo/bar").expect("Should parse URI with no host");
        assert_eq!("/foo/bar", uri.path());
        // Here's why: https://github.com/whatwg/url/pull/544
        assert!(uri.host().is_none());

        let uri =
            url::Url::parse("foo://localhost/foo/bar").expect("Should parse URI with no host");
        assert_eq!("/foo/bar", uri.path());
        assert_eq!(uri.host_str(), Some("localhost"));

        let uri =
            url::Url::parse("bindle:localhost/foo/bar").expect("Should parse URI with no host");
        assert_eq!("localhost/foo/bar", uri.path());
        assert!(uri.host().is_none());

        // Two from the Bindle spec
        let uri = url::Url::parse("bindle:example.com/hello_world/1.2.3")
            .expect("Should parse URI with no host");
        assert_eq!("example.com/hello_world/1.2.3", uri.path());
        assert!(uri.host().is_none());

        let uri = url::Url::parse(
            "bindle:github.com/deislabs/example_bindle/123.234.34567-alpha.9999+hellothere",
        )
        .expect("Should parse URI with no host");
        assert_eq!(
            "github.com/deislabs/example_bindle/123.234.34567-alpha.9999+hellothere",
            uri.path()
        );
        assert!(uri.host().is_none());
    }

    #[test]
    fn test_url_to_oci() {
        let uri = url::Url::parse("oci:foo:bar").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("foo:bar", oci.whole().as_str());

        let uri = url::Url::parse("oci://example.com/foo:dev").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("example.com/foo:dev", oci.whole().as_str());

        let uri = url::Url::parse("oci:example/foo:1.2.3").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("example/foo:1.2.3", oci.whole().as_str());

        let uri = url::Url::parse("oci://example.com/foo:dev").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("example.com/foo:dev", oci.whole().as_str());

        let uri = url::Url::parse("oci://example.com:9000/foo:dev").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("example.com:9000/foo:dev", oci.whole().as_str());
    }

    #[test]
    fn test_parse_host_header_uri() {
        let module = Module::new("/base".to_string(), "file:///no/such/path.wasm".to_owned());

        let hmap = |val: &str| {
            let mut hm = hyper::HeaderMap::new();
            hm.insert(
                "HOST",
                hyper::header::HeaderValue::from_str(val).expect("Made a header value"),
            );
            hm
        };

        let default_host = "example.com:1234";

        {
            // All should come from HOST header
            let headers = hmap("wagi.net:31337");
            let uri = hyper::Uri::from_str("http://localhost:443/foo/bar").expect("parsed URI");

            let (host, port) = module.parse_host_header_uri(&headers, &uri, default_host);
            assert_eq!("wagi.net", host);
            assert_eq!("31337", port);
        }
        {
            // Name should come from HOST, port should come from self.host
            let headers = hmap("wagi.net");
            let uri = hyper::Uri::from_str("http://localhost:443/foo/bar").expect("parsed URI");

            let (host, port) = module.parse_host_header_uri(&headers, &uri, default_host);
            assert_eq!("wagi.net", host);
            assert_eq!("1234", port)
        }
        {
            // Host and domain should come from default_host
            let headers = hyper::HeaderMap::new();
            let uri = hyper::Uri::from_str("http://localhost:8080/foo/bar").expect("parsed URI");

            let (host, port) = module.parse_host_header_uri(&headers, &uri, default_host);

            assert_eq!("example.com", host);
            assert_eq!("1234", port)
        }
        {
            // Host and port should come from URI
            let empty_host = "";
            let headers = hyper::HeaderMap::new();
            let uri = hyper::Uri::from_str("http://localhost:8080/foo/bar").expect("parsed URI");

            let (host, port) = module.parse_host_header_uri(&headers, &uri, empty_host);

            assert_eq!("localhost", host);
            assert_eq!("8080", port)
        }
    }

    #[test]
    fn test_headers() {
        let module = Module::new(
            "/path/...".to_string(),
            "file:///no/such/path.wasm".to_owned(),
        );
        let (req, _) = Request::builder()
            .uri("https://example.com:3000/path/test?foo=bar")
            .header("X-Test-Header", "hello")
            .header("Accept", "text/html")
            .header("User-agent", "test")
            .header("Host", "example.com:3000")
            .header("Authorization", "supersecret")
            .header("Connection", "sensitive")
            .method("POST")
            .body(())
            .unwrap()
            .into_parts();
        let content_length = 1234;
        let client_addr = "192.168.0.1:3000".parse().expect("Should parse IP");
        let default_host = "example.com:3000";
        let use_tls = true;
        let env = std::collections::HashMap::with_capacity(0);
        let headers = module.build_headers(
            &req,
            content_length,
            client_addr,
            default_host,
            use_tls,
            env,
        );

        let want = |key: &str, expect: &str| {
            let v = headers
                .get(&key.to_owned())
                .unwrap_or_else(|| panic!("expected to find key {}", key));

            assert_eq!(expect, v)
        };

        // Content-type is set on output, so we don't test here.
        want("X_MATCHED_ROUTE", "/path/...");
        want("HTTP_ACCEPT", "text/html");
        want("REQUEST_METHOD", "POST");
        want("SERVER_PROTOCOL", "HTTP/1.1");
        want("HTTP_USER_AGENT", "test");
        want("SCRIPT_NAME", "file:///no/such/path.wasm");
        want("SERVER_SOFTWARE", "WAGI/1");
        want("SERVER_PORT", "3000");
        want("SERVER_NAME", "example.com");
        want("AUTH_TYPE", "");
        want("REMOTE_ADDR", "192.168.0.1");
        want("REMOTE_ADDR", "192.168.0.1");
        want("PATH_INFO", "/path/test");
        want("QUERY_STRING", "foo=bar");
        want("PATH_TRANSLATED", "/path/test");
        want("CONTENT_LENGTH", "1234");
        want("HTTP_HOST", "example.com:3000");
        want("GATEWAY_INTERFACE", "CGI/1.1");
        want("REMOTE_USER", "");
        want("X_FULL_URL", "https://example.com:3000/path/test?foo=bar");
        want("X_RELATIVE_PATH", "test");

        // Extra header should be passed through
        want("HTTP_X_TEST_HEADER", "hello");

        // Finally, security-sensitive headers should be removed.
        assert!(headers.get("HTTP_AUTHORIZATION").is_none());
        assert!(headers.get("HTTP_CONNECTION").is_none());
    }
}
