use hyper::{
    http::header::{HeaderName, HeaderValue},
    http::request::Parts,
    http::uri::Scheme,
    Body, Request, Response, StatusCode,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};
use wasi_common::virtfs::pipe::{ReadPipe, WritePipe};
use wasmtime::*;
use wasmtime_wasi::{Wasi, WasiCtxBuilder};

/// This sets the version of CGI that WAGI adheres to.
///
/// At the point at which WAGI diverges from CGI, this value will be replaced with
/// WAGI/1.0
const WAGI_VERSION: &str = "CGI/1.1";
const SERVER_SOFTWARE_VERSION: &str = "WAGI/1";

pub struct Router {
    //pub config_path: String,
    pub module_config: ModuleConfig,
}

impl Router {
    /// Route the request to the correct handler
    ///
    /// Some routes are built in (like healthz), while others are dynamically
    /// dispatched.
    pub async fn route(&self, req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
        // TODO: THis should be refactored into a Router that loads the TOML file
        // (optionally only at startup) and then routes directly. Right now, each
        // request is causing the TOML file to be read and parsed anew. This is great
        // for debugging (since we can edit the TOML without restarting), but it does
        // incur a performance penalty.
        //
        // Additionally, we could implement an LRU to cache WASM modules. This would
        // greatly reduce the amount of load time per request. But this would come with two
        // drawbacks: (a) it would be different than CGI, and (b) it would involve a cache
        // clear during debugging, which could be a bit annoying.

        let uri_path = req.uri().path();
        match uri_path {
            "/healthz" => Ok(Response::new(Body::from("OK"))),
            _ => match self.find_wasm_module(uri_path) {
                Ok(module) => Ok(module.execute(req).await),
                Err(e) => {
                    eprintln!("error: {}", e);
                    Ok(not_found())
                }
            },
        }
    }
    /// Load the configuration TOML and find a module that matches
    fn find_wasm_module(&self, uri_path: &str) -> Result<Module, anyhow::Error> {
        //let config = self.module_config; //self.load_modules_toml()?;
        let found = self
            .module_config
            .module
            .iter()
            .filter(|m| m.match_route(uri_path))
            .last();
        if found.is_none() {
            return Err(anyhow::anyhow!("module not found: {}", uri_path));
        }

        let found_mod = (*found.unwrap()).clone();
        Ok(found_mod)
    }
}

/// The configuration for all modules in a WAGI site
#[derive(Clone, Deserialize)]
pub struct ModuleConfig {
    pub module: Vec<Module>,
}

/// Description of a single WAGI module
#[derive(Clone, Deserialize)]
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
    // The key (left value) is the name of the directory INSIDE the WASM. The value is
    // the location OUTSIDE the WASM. Two inside locations can map to the same outside
    // location.
    pub volumes: Option<HashMap<String, String>>,
    // Set additional environment variables
    pub environment: Option<HashMap<String, String>>,
}

impl Module {
    /// Execute the WASM module in a WAGI
    async fn execute(&self, req: Request<Body>) -> Response<Body> {
        // Read the parts in here
        let (parts, body) = req.into_parts();
        let data = hyper::body::to_bytes(body)
            .await
            .unwrap_or_default()
            .to_vec();

        match self.run_wasm(&parts, data) {
            Ok(res) => res,
            Err(e) => {
                eprintln!("error running WASM module: {}", e);
                // A 500 error makes sense here
                let mut srv_err = Response::default();
                *srv_err.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                srv_err
            }
        }
    }
    /// Check whether the given fragment matches the route in this module.
    ///
    /// A route matches if
    ///   - the module route is a literal path, and the fragment is an exact match
    ///   - the module route ends with '/...' and the portion before that is an exact
    ///     match with the start of the fragment (e.g. /foo/... matches /foo/bar/foo)
    ///
    /// Note that the route /foo/... matches the URI path /foo.
    fn match_route(&self, fragment: &str) -> bool {
        self.route
            .strip_suffix("/...")
            .map(|i| fragment.starts_with(i))
            .unwrap_or_else(|| self.route.as_str() == fragment)
    }

    fn build_headers(&self, req: &Parts, content_length: usize) -> HashMap<String, String> {
        // Note that we put these first so that there is no chance that they overwrite
        // the built-in vars. IMPORTANT: This is also why some values have empty strings
        // deliberately set (as opposed to omiting the pair altogether).
        let mut headers = self.environment.clone().unwrap_or_default();

        let host = req
            .headers
            .get("HOST")
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
        headers.insert("REMOTE_ADDR".to_owned(), "127.0.0.1".to_owned()); // TODO: Where can we get the client IP?
        headers.insert("REMOTE_HOST".to_owned(), "localhost".to_owned()); // TODO: Where can we get the client host?
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
    fn run_wasm(&self, req: &Parts, body: Vec<u8>) -> Result<Response<Body>, anyhow::Error> {
        let store = Store::default();
        let mut linker = Linker::new(&store);
        let uri_path = req.uri.path();

        let headers = self.build_headers(req, body.len());

        let stdin = ReadPipe::from(body);

        let stdout_buf: Vec<u8> = vec![];
        let stdout_mutex = Arc::new(RwLock::new(stdout_buf));
        let stdout = WritePipe::from_shared(stdout_mutex.clone());
        // The spec does not say what to do with STDERR.
        // See specifically sections 4.2 and 6.1 of RFC 3875.
        // Currently, we will attach to wherever logs go.

        let mut args = vec![uri_path];
        req.uri
            .query()
            .map(|q| q.split('&').for_each(|item| args.push(item)))
            .take();

        let mut builder = WasiCtxBuilder::new();
        builder
            .args(args)
            .envs(headers)
            .inherit_stderr() // STDERR goes to the console of the server
            .stdout(stdout) // STDOUT is sent to a Vec<u8>, which becomes the Body later
            .stdin(stdin);

        // Map all of the volumes.
        if let Some(dirs) = self.volumes.as_ref() {
            for (guest, host) in dirs.iter() {
                // Try to open the dir or log an error.
                match std::fs::File::open(host) {
                    Ok(dir) => {
                        builder.preopened_dir(dir, guest);
                    }
                    Err(e) => eprintln!("Error opening {} -> {}: {}", host, guest, e),
                }
            }
        }

        let ctx = builder.build()?;
        let wasi = Wasi::new(&store, ctx);
        wasi.add_to_linker(&mut linker)?;

        let module = wasmtime::Module::from_file(store.engine(), self.module.as_str())?;
        let instance = linker.instantiate(&module)?;

        // Typically, the function we execute for WASI is "_start".
        let start = instance.get_func("_start").unwrap().get0::<()>()?;
        start()?;

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
                            println!("Setting status to {}", status);
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
                            Err(e) => eprintln!("Invalid header name '{}': {}", h.0.as_str(), e),
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
}

/// Create an HTTP 404 response
fn not_found() -> Response<Body> {
    let mut not_found = Response::default();
    *not_found.status_mut() = StatusCode::NOT_FOUND;
    not_found
}

/// Create an HTTP 500 response
fn internal_error(msg: &str) -> Response<Body> {
    println!("HTTP 500 error: {}", msg);
    let mut res = Response::new(Body::from(msg.to_owned()));
    *res.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    res
}

fn parse_cgi_headers(headers: String) -> HashMap<String, String> {
    let mut map = HashMap::new();
    headers.trim().split('\n').for_each(|h| {
        let parts: Vec<&str> = h.splitn(2, ':').collect();
        if parts.len() != 2 {
            println!("corrupt header: {}", h);
            return;
        }
        map.insert(parts[0].trim().to_owned(), parts[1].trim().to_owned());
    });
    map
}

/// Load the configuration TOML
pub fn load_modules_toml(filename: &str) -> Result<ModuleConfig, anyhow::Error> {
    if !Path::new(filename).is_file() {
        return Err(anyhow::anyhow!(
            "no modules configuration file found at {}",
            filename
        ));
    }

    let data = std::fs::read_to_string(filename)?;
    let modules: ModuleConfig = toml::from_str(data.as_str())?;
    Ok(modules)
}
