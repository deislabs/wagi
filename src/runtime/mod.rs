//! The tools for executing WAGI modules, and managing the lifecycle of a request.

use cap_std::fs::Dir;
use hyper::{
    header::HOST,
    http::header::{HeaderName, HeaderValue},
    http::request::Parts,
    http::uri::Scheme,
    Body, Request, Response, StatusCode,
};
use log::debug;
use oci_distribution::client::{Client, ClientConfig};
use oci_distribution::secrets::RegistryAuth;
use oci_distribution::Reference;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use std::{collections::HashMap, net::SocketAddr};
use url::Url;
use wasi_cap_std_sync::WasiCtxBuilder;
use wasi_common::pipe::{ReadPipe, WritePipe};
use wasmtime::*;
use wasmtime_wasi::Wasi;

use crate::version::*;
use crate::{http_util::*, runtime::bindle::bindle_cache_key};

pub mod bindle;

/// The default Bindle server URL.
pub const DEFAULT_BINDLE_SERVER: &str = "http://localhost:8080/v1";

const WASM_LAYER_CONTENT_TYPE: &str = "application/vnd.wasm.content.layer.v1+wasm";

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

    pub fn host(&self) -> Option<&String> {
        self.module.host.as_ref()
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
    /// Set additional environment variables
    pub environment: Option<HashMap<String, String>>,
    /// The name of the function that is the entrypoint for executing the module.
    /// The default is `_start`.
    pub entrypoint: Option<String>,
    /// The name of the host.
    pub host: Option<String>,
    /// The URL fragment for the bindle server.
    ///
    /// If none is supplied, then http://localhost:8080/v1 is used
    pub bindle_server: Option<String>,

    /// List of hosts that the guest module is allowed to make HTTP requests to.
    /// If none or an empty vector is supplied, the guest module cannot send
    /// requests to any server.
    pub allowed_hosts: Option<Vec<String>>,
}

impl Module {
    /// Execute the WASM module in a WAGI
    pub async fn execute(
        &self,
        entrypoint: &str,
        req: Request<Body>,
        client_addr: SocketAddr,
        cache_config_path: String,
        module_cache_dir: PathBuf,
    ) -> Response<Body> {
        // Read the parts in here
        log::trace!(
            "Module::execute: route={}, module={}",
            self.route,
            self.module
        );

        let (parts, body) = req.into_parts();
        let data = hyper::body::to_bytes(body)
            .await
            .unwrap_or_default()
            .to_vec();
        let ep = entrypoint.to_owned();
        let me = self.clone();
        let res = match tokio::task::spawn_blocking(move || {
            me.run_wasm(
                &ep,
                &parts,
                data,
                client_addr,
                cache_config_path,
                module_cache_dir,
            )
        })
        .await
        {
            Ok(res) => res,
            Err(e) if e.is_panic() => {
                log::error!("Recoverable panic on Wasm Runner thread: {}", e);
                return internal_error("Module run error");
            }
            Err(e) => {
                log::error!("Recoverable panic on Wasm Runner thread: {}", e);
                return internal_error("module run was cancelled");
            }
        };
        match res {
            Ok(res) => res,
            Err(e) => {
                log::error!("error running WASM module: {}", e);
                // A 500 error makes sense here
                let mut srv_err = Response::default();
                *srv_err.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                srv_err
            }
        }
    }

    /// Examine the given module to see if it has any routes.
    ///
    /// If it has any routes, add them to the vector and return it.
    pub(crate) fn load_routes(
        &self,
        cache_config_path: String,
        module_cache_dir: PathBuf,
    ) -> Result<Vec<RouteEntry>, anyhow::Error> {
        let start_time = Instant::now();

        let prefix = self
            .route
            .strip_suffix("/...")
            .unwrap_or(self.route.as_str());
        let mut routes = vec![];

        routes.push(RouteEntry {
            path: self.route.to_owned(), // We don't use prefix because prefix has been normalized.
            entrypoint: self.entrypoint.clone().unwrap_or("_start".to_string()),
        });

        let store = self.new_store(cache_config_path)?;
        let mut linker = Linker::new(&store);
        let stdout_buf: Vec<u8> = vec![];
        let stdout_mutex = Arc::new(RwLock::new(stdout_buf));
        let stdout = WritePipe::from_shared(stdout_mutex.clone());
        let mut builder = WasiCtxBuilder::new();

        builder = builder
            .inherit_stderr() // STDERR goes to the console of the server
            .stdout(Box::new(stdout));

        let ctx = builder.build()?;
        let wasi = Wasi::new(&store, ctx);
        wasi.add_to_linker(&mut linker)?;
        let http = wasi_experimental_http_wasmtime::HttpCtx::new(None, None)?;
        http.add_to_linker(&mut linker)?;

        let module = self.load_cached_module(&store, module_cache_dir)?;
        let instance = linker.instantiate(&module)?;

        let duration = start_time.elapsed();
        log::info!(
            "(load_routes) instantiation time for module {}: {:#?}",
            self.module.as_str(),
            duration
        );

        match instance.get_func("_routes") {
            Some(func) => {
                func.call(&[])?;
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
    ) -> HashMap<String, String> {
        // Note that we put these first so that there is no chance that they overwrite
        // the built-in vars. IMPORTANT: This is also why some values have empty strings
        // deliberately set (as opposed to omiting the pair altogether).
        let mut headers = self.environment.clone().unwrap_or_default();

        let host = req
            .headers
            .get(HOST)
            .map(|val| val.to_str().unwrap_or("localhost"))
            .unwrap_or("localhost")
            .to_owned();

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

        // Since this is not in the specification, an X_ is prepended, per spec.
        // NB: It is strange that there is not a way to do this already. The Display impl
        // seems to only provide the path.
        let uri = req.uri.clone();
        headers.insert(
            "X_FULL_URL".to_owned(),
            format!(
                "{}://{}{}",
                uri.scheme_str().unwrap_or("http"), // It is not clear if Hyper ever sets scheme.
                uri.authority()
                    .map(|a| a.as_str())
                    .unwrap_or_else(|| host.as_str()),
                uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("")
            ),
        );

        headers.insert("GATEWAY_INTERFACE".to_owned(), WAGI_VERSION.to_owned());
        headers.insert("X_MATCHED_ROUTE".to_owned(), self.route.to_owned()); // Specific to WAGI (not CGI)
        headers.insert("PATH_INFO".to_owned(), req.uri.path().to_owned()); // TODO: Does this get trimmed?

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
        headers.insert(
            "SERVER_PORT".to_owned(),
            req.uri
                .port()
                .map(|p| p.to_string())
                .unwrap_or_else(|| "80".to_owned()),
        );
        headers.insert(
            "SERVER_PROTOCOL".to_owned(),
            req.uri
                .scheme()
                .unwrap_or(&Scheme::HTTP)
                .as_str()
                .to_owned(),
        );

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
            if key == "HTTP_AUHTORIZATION" || key == "HTTP_CONNECTION" {
                return;
            }
            let val = header.1.to_str().unwrap_or("CORRUPT VALUE").to_owned();
            headers.insert(key, val);
        });

        headers
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
    fn run_wasm(
        &self,
        entrypoint: &str,
        req: &Parts,
        body: Vec<u8>,
        client_addr: SocketAddr,
        cache_config_path: String,
        cache_dir: PathBuf,
    ) -> Result<Response<Body>, anyhow::Error> {
        log::trace!(
            "Moduke::run_wasm: uri={}, module={}, entrypoint={}",
            req.uri,
            &self.module,
            &entrypoint
        );

        let start_time = Instant::now();

        let store = self.new_store(cache_config_path)?;
        let mut linker = Linker::new(&store);
        let uri_path = req.uri.path();

        let headers = self.build_headers(req, body.len(), client_addr);

        let stdin = ReadPipe::from(body);

        let stdout_buf: Vec<u8> = vec![];
        let stdout_mutex = Arc::new(RwLock::new(stdout_buf));
        let stdout = WritePipe::from_shared(stdout_mutex.clone());
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

        let mut builder = WasiCtxBuilder::new();
        builder = builder
            .args(&args)?
            .envs(&headers)?
            .inherit_stderr() // STDERR goes to the console of the server
            .stdout(Box::new(stdout)) // STDOUT is sent to a Vec<u8>, which becomes the Body later
            .stdin(Box::new(stdin));

        // Map all of the volumes.
        if let Some(dirs) = self.volumes.as_ref() {
            for (guest, host) in dirs.iter() {
                debug!("Mapping volume from {} (host) to {} (guest)", host, guest);
                // Try to open the dir or log an error.
                match unsafe { Dir::open_ambient_dir(host) } {
                    Ok(dir) => {
                        builder = builder.preopened_dir(dir, guest)?;
                    }
                    Err(e) => log::error!("Error opening {} -> {}: {}", host, guest, e),
                };
            }
        }

        let ctx = builder.build()?;
        let wasi = Wasi::new(&store, ctx);
        wasi.add_to_linker(&mut linker)?;

        let http = wasi_experimental_http_wasmtime::HttpCtx::new(self.allowed_hosts.clone(), None)?;
        http.add_to_linker(&mut linker)?;

        //let module = wasmtime::Module::from_file(store.engine(), self.module.as_str())?;
        let module = self.load_cached_module(&store, cache_dir)?;
        let instance = linker.instantiate(&module)?;

        let duration = start_time.elapsed();
        log::info!(
            "instantiation time for module {}: {:#?}",
            self.module.as_str(),
            duration
        );

        // This shouldn't error out, because we already know there is a match.
        let start = instance.get_func(entrypoint).ok_or_else(|| {
            anyhow::anyhow!(
                "No such function '{}' in {}",
                entrypoint.clone(),
                self.module
            )
        })?;

        log::trace!("Module::run_wasm: calling Wasm entry point");
        start.call(&[])?;

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
                            log::info!("Setting status to {}", status);
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
                            Err(e) => log::error!("Invalid header name '{}': {}", h.0.as_str(), e),
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
    async fn load_module(&self, store: &Store, cache: PathBuf) -> anyhow::Result<wasmtime::Module> {
        log::trace!(
            "Module::load_module: loading from source: module={}",
            &self.module
        );
        match Url::parse(self.module.as_str()) {
            Err(e) => {
                log::debug!(
                    "Error parsing module URI {}. Assuming this is a local file.",
                    e
                );
                wasmtime::Module::from_file(store.engine(), self.module.as_str())
            }
            Ok(uri) => match uri.scheme() {
                "file" => wasmtime::Module::from_file(store.engine(), uri.path()),
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
    fn load_cached_module(
        &self,
        store: &Store,
        cache_dir: PathBuf,
    ) -> anyhow::Result<wasmtime::Module> {
        log::trace!("Module::load_cached_module: {}", &self.module);
        let canonical_path = match Url::parse(self.module.as_str()) {
            Err(e) => {
                log::debug!(
                    "Error parsing module URI {}. Assuming this is a local file.",
                    e
                );
                PathBuf::from(self.module.as_str())
            }
            Ok(uri) => match uri.scheme() {
                "file" => PathBuf::from(uri.path()),
                "bindle" => cache_dir.join(bindle_cache_key(&uri)),
                "parcel" => cache_dir.join(uri.path()), // parcel:SHA256 becomes cache_dir/SHA256
                "oci" => cache_dir.join(self.hash_name()),
                s => {
                    log::error!(
                        "Module::load_cached_module: unknown scheme {} in module {}",
                        s,
                        &self.module
                    );
                    anyhow::bail!("Unknown scheme {}", s)
                }
            },
        };
        log::trace!(
            "Module::load_cached_module: canonical_path={:?}",
            canonical_path
        );

        // If there is a module at this path, load it.
        // Right now, _any_ problem loading the module will result in us trying to
        // re-fetch it.
        match wasmtime::Module::from_file(store.engine(), canonical_path) {
            Ok(module) => Ok(module),
            Err(_e) => {
                log::debug!(
                    "module cache miss. Loading module {} from remote.",
                    self.module
                );
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

    async fn load_bindle(
        &self,
        uri: &Url,
        engine: &Engine,
        cache: PathBuf,
    ) -> anyhow::Result<wasmtime::Module> {
        log::trace!(
            "Module::load_bindle: server={:?}, uri={}",
            self.bindle_server,
            uri
        );
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
    async fn load_parcel(
        &self,
        uri: &Url,
        engine: &Engine,
        cache: PathBuf,
    ) -> anyhow::Result<wasmtime::Module> {
        let bs = self
            .bindle_server
            .clone()
            .unwrap_or_else(|| DEFAULT_BINDLE_SERVER.to_owned());
        bindle::load_parcel(bs.as_str(), uri, engine, cache).await
    }
    async fn load_oci(
        &self,
        uri: &Url,
        engine: &Engine,
        cache: PathBuf,
    ) -> anyhow::Result<wasmtime::Module> {
        log::trace!("Module::load_oci: uri={}", uri);
        let mut config = ClientConfig::default();
        config.protocol = oci_distribution::client::ClientProtocol::HttpsExcept(vec![
            "localhost:5000".to_owned(),
            "127.0.0.1:5000".to_owned(),
        ]);
        let mut oc = Client::new(config);
        let auth = RegistryAuth::Anonymous;

        let img = url_to_oci(uri).map_err(|e| {
            log::error!(
                "Module::load_oci: could not convert {} to OCI reference: {}",
                uri,
                e
            );
            e
        })?;
        let data = oc
            .pull(&img, &auth, vec![WASM_LAYER_CONTENT_TYPE])
            .await
            .map_err(|e| {
                log::error!("Module::load_oci: pull failed: {}", e);
                e
            })?;
        if data.layers.is_empty() {
            log::error!("Module::load_oci: image {} has no layers", &img);
            anyhow::bail!("image has no layers");
        }
        let first_layer = data.layers.get(0).unwrap();

        // If a cache write fails, log it but continue on.
        log::trace!("Module::load_oci: writing layer to module cache");
        tokio::fs::write(cache.join(self.hash_name()), first_layer.data.as_slice())
            .await
            .err()
            .map(|e| log::warn!("failed to write module to cache: {}", e));
        let module = wasmtime::Module::new(engine, first_layer.data.as_slice())?;
        Ok(module)
    }

    fn new_store(&self, cache_config_path: String) -> Result<Store, anyhow::Error> {
        let mut config = Config::default();

        if let Ok(p) = std::fs::canonicalize(cache_config_path) {
            config.cache_config_load(p)?;
        };

        let engine = Engine::new(&config)?;
        Ok(Store::new(&engine))
    }
}

/// Build the image name from the URL passed in.
/// So oci://example.com/foo:latest will become example.com/foo:latest
///
/// If parsing fails, this will emit an error.
fn url_to_oci(uri: &Url) -> anyhow::Result<Reference> {
    let name = uri.path().trim_start_matches("/");
    let port = uri
        .port()
        .and_then(|p| Some(format!(":{}", p)))
        .unwrap_or_else(|| "".to_owned());
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
    use crate::DEFAULT_HOST as LOCALHOST;

    use std::io::Write;
    use tempfile::NamedTempFile;

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
        let watfile = tf.path();

        // HEY RADU! IS THIS OKAY FOR A TEST?
        let cache = "cache.toml".to_string();

        let module = Module {
            route: "/base".to_string(),
            module: watfile.to_string_lossy().to_string(),
            volumes: None,
            environment: None,
            entrypoint: None,
            host: None,
            bindle_server: None,
            allowed_hosts: None,
        };

        // We should be able to mount the same wasm at a separate route.
        let module2 = Module {
            route: "/another/...".to_string(),
            module: watfile.to_string_lossy().to_string(),
            volumes: None,
            environment: None,
            entrypoint: None,
            host: None,
            bindle_server: None,
            allowed_hosts: None,
        };

        let mut mc = ModuleConfig {
            modules: vec![module.clone(), module2.clone()],
            route_cache: None,
            default_host: None,
        };

        mc.build_registry(
            cache,
            tempfile::tempdir().expect("new temp dir").into_path(),
        )
        .await
        .expect("registry build cleanly");

        log::debug!("{:#?}", mc.route_cache);

        // Three routes for each module.
        assert_eq!(6, mc.route_cache.as_ref().expect("routes are set").len());

        let modpath = module.module.clone();

        // Base route is from the config file
        let base = mc
            .handler_for_host_path(LOCALHOST, "/base")
            .expect("Should get a /base route");
        assert_eq!("_start", base.entrypoint);
        assert_eq!(modpath, base.module.module);

        // Route one is from the module's _routes()
        let one = mc
            .handler_for_host_path(LOCALHOST, "/base/one")
            .expect("Should get the /base/one route");

        assert_eq!("one", one.entrypoint);
        assert_eq!(modpath, one.module.module);

        // Route two is a wildcard.
        let two = mc
            .handler_for_host_path(LOCALHOST, "/base/two/three")
            .expect("Should get the /base/two/... route");

        assert_eq!("two", two.entrypoint);
        assert_eq!(modpath, two.module.module);

        // This should fail
        assert!(mc
            .handler_for_host_path(LOCALHOST, "/base/no/such/path")
            .is_err());

        // This should pass
        mc.handler_for_host_path(LOCALHOST, "/another/path")
            .expect("The generic handler should have been returned for this");
    }

    #[tokio::test]
    async fn should_override_default_domain() {
        let tf = write_temp_wat(ROUTES_WAT).expect("wrote tempfile");
        let watfile = tf.path();
        let cache = "cache.toml".to_string();

        let module = Module {
            route: "/base".to_string(),
            module: watfile.to_string_lossy().to_string(),
            volumes: None,
            environment: None,
            entrypoint: None,
            host: None,
            bindle_server: None,
            allowed_hosts: None,
        };

        let mut mc = ModuleConfig {
            modules: vec![module.clone()],
            route_cache: None,
            default_host: Some("localhost.localdomain".to_owned()),
        };

        mc.build_registry(
            cache,
            tempfile::tempdir().expect("new temp dir").into_path(),
        )
        .await
        .expect("registry build cleanly");

        // This should fail b/c default domain is localhost.localdomain
        assert!(mc.handler_for_host_path("localhost", "/base").is_err());

        assert!(mc
            .handler_for_host_path("localhost.localdomain", "/base")
            .is_ok())
    }

    #[tokio::test]
    async fn should_parse_file_uri() {
        let tf = write_temp_wat(ROUTES_WAT).expect("wrote tempfile");
        let watfile = tf.path();

        let urlish = format!("file:{}", watfile.to_string_lossy().to_string());
        println!("Testing URL: {}", urlish);

        let module = Module {
            route: "/base".to_string(),
            module: urlish,
            volumes: None,
            environment: None,
            entrypoint: None,
            host: None,
            bindle_server: None,
            allowed_hosts: None,
        };

        let store = super::Store::default();

        module
            .load_module(
                &store,
                tempfile::tempdir().expect("create a temp dir").into_path(),
            )
            .await
            .expect("loaded module");
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
}
