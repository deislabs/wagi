//! The tools for executing WAGI modules, and managing the lifecycle of a request.

use hyper::{
    header::HOST,
    http::header::{HeaderName, HeaderValue},
    http::request::Parts,
    http::uri::Scheme,
    Body, Request, Response, StatusCode,
};
use serde::Deserialize;
use std::io::BufRead;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use std::{collections::HashMap, net::SocketAddr};
use wasi_common::virtfs::pipe::{ReadPipe, WritePipe};
use wasmtime::*;
use wasmtime_wasi::{Wasi, WasiCtxBuilder};

use crate::http_util::*;
use crate::version::*;

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
    pub host: Option<String>,
}

impl Handler {
    /// Given a module and a route entry, create a new handler.
    pub fn new(entry: &RouteEntry, module: &Module) -> Self {
        Handler {
            path: entry.path.clone(),
            entrypoint: entry.entrypoint.clone(),
            module: module.clone(),
            host: module.host.clone(),
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
    /// Set additional environment variables
    pub environment: Option<HashMap<String, String>>,
    /// The name of the function that is the entrypoint for executing the module.
    /// The default is `_start`.
    pub entrypoint: Option<String>,
    /// The name of the host.
    pub host: Option<String>,
}

impl Module {
    /// Execute the WASM module in a WAGI
    pub async fn execute(
        &self,
        entrypoint: &str,
        req: Request<Body>,
        client_addr: SocketAddr,
        cache_config_path: String,
    ) -> Response<Body> {
        // Read the parts in here
        let (parts, body) = req.into_parts();
        let data = hyper::body::to_bytes(body)
            .await
            .unwrap_or_default()
            .to_vec();

        match self.run_wasm(entrypoint, &parts, data, client_addr, cache_config_path) {
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

    /// Examine the given module to see if it has any routes.
    ///
    /// If it has any routes, add them to the vector and return it.
    pub fn load_routes(&self, cache_config_path: String) -> Result<Vec<RouteEntry>, anyhow::Error> {
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

        let store = match std::fs::canonicalize(cache_config_path) {
            Ok(p) => {
                let mut engine_config = Config::default();
                engine_config.cache_config_load(p)?;
                let engine = Engine::new(&engine_config);
                Store::new(&engine)
            }
            Err(_) => Store::default(),
        };
        let mut linker = Linker::new(&store);
        let stdout_buf: Vec<u8> = vec![];
        let stdout_mutex = Arc::new(RwLock::new(stdout_buf));
        let stdout = WritePipe::from_shared(stdout_mutex.clone());
        let mut builder = WasiCtxBuilder::new();
        builder
            .inherit_stderr() // STDERR goes to the console of the server
            .stdout(stdout);

        let ctx = builder.build()?;
        let wasi = Wasi::new(&store, ctx);
        wasi.add_to_linker(&mut linker)?;

        let module = wasmtime::Module::from_file(store.engine(), self.module.as_str())?;
        let instance = linker.instantiate(&module)?;

        let duration = start_time.elapsed();
        println!(
            "(load_routes) instantiation time for module {}: {:#?}",
            self.module.as_str(),
            duration
        );

        match instance.get_func("_routes") {
            Some(func) => {
                let start = func.get0::<()>()?;
                start()?;
            }
            None => return Ok(routes),
        }

        let out = stdout_mutex.read().unwrap();
        out.lines().for_each(|line_result| {
            if let Ok(line) = line_result {
                // Split line into parts
                let parts: Vec<&str> = line.trim().split_whitespace().collect();

                if parts.len() == 0 {
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
    ) -> Result<Response<Body>, anyhow::Error> {
        let start_time = Instant::now();
        let store = match std::fs::canonicalize(cache_config_path.clone()) {
            Ok(p) => {
                let mut engine_config = Config::default();
                engine_config.cache_config_load(p)?;
                let engine = Engine::new(&engine_config);
                Store::new(&engine)
            }
            Err(_) => Store::default(),
        };

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

        let duration = start_time.elapsed();
        println!(
            "instantiation time for module {}: {:#?}",
            self.module.as_str(),
            duration
        );

        // This shouldn't error out, because we already know there is a match.
        let start = instance
            .get_func(entrypoint)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No such function '{}' in {}",
                    entrypoint.clone(),
                    self.module
                )
            })?
            .get0::<()>()?;
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

#[cfg(test)]
mod test {
    use crate::ModuleConfig;

    use super::Module;
    use std::io::Write;

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

    #[test]
    fn load_routes_from_wasm() {
        let mut tf = tempfile::NamedTempFile::new().expect("create a temp file");
        write!(tf, "{}", ROUTES_WAT).expect("wrote WAT to disk");
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
        };

        // We should be able to mount the same wasm at a separate route.
        let module2 = Module {
            route: "/another/...".to_string(),
            module: watfile.to_string_lossy().to_string(),
            volumes: None,
            environment: None,
            entrypoint: None,
            host: None,
        };

        let mut mc = ModuleConfig {
            modules: vec![module.clone(), module2.clone()],
            route_cache: None,
            default_host: None,
        };

        mc.build_registry(cache).expect("registry build cleanly");

        println!("{:#?}", mc.route_cache);

        // Three routes for each module.
        assert_eq!(6, mc.route_cache.as_ref().expect("routes are set").len());

        let modpath = module.module.clone();

        // Base route is from the config file
        let base = mc
            .handler_for_host_path("localhost", "/base")
            .expect("Should get a /base route");
        assert_eq!("_start", base.entrypoint);
        assert_eq!(modpath, base.module.module);

        // Route one is from the module's _routes()
        let one = mc
            .handler_for_host_path("localhost", "/base/one")
            .expect("Should get the /base/one route");

        assert_eq!("one", one.entrypoint);
        assert_eq!(modpath, one.module.module);

        // Route two is a wildcard.
        let two = mc
            .handler_for_host_path("localhost", "/base/two/three")
            .expect("Should get the /base/two/... route");

        assert_eq!("two", two.entrypoint);
        assert_eq!(modpath, two.module.module);

        // This should fail
        assert!(mc
            .handler_for_host_path("localhost", "/base/no/such/path")
            .is_err());

        // This should pass
        mc.handler_for_host_path("localhost", "/another/path")
            .expect("The generic handler should have been returned for this");
    }

    #[test]
    fn should_override_default_domain() {
        let mut tf = tempfile::NamedTempFile::new().expect("create a temp file");
        write!(tf, "{}", ROUTES_WAT).expect("wrote WAT to disk");
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
        };

        let mut mc = ModuleConfig {
            modules: vec![module.clone()],
            route_cache: None,
            default_host: Some("localhost.localdomain".to_owned()),
        };

        mc.build_registry(cache).expect("registry build cleanly");

        // This should fail b/c default domain is localhost.localdomain
        assert!(mc.handler_for_host_path("localhost", "/base").is_err());

        assert!(mc
            .handler_for_host_path("localhost.localdomain", "/base")
            .is_ok())
    }
}
