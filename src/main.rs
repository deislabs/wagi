use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use serde::Deserialize;
use wasmtime::*;
use wasmtime_wasi::{Wasi, WasiCtxBuilder};

#[tokio::main]
pub async fn main() {
    println!("=> Starting server");
    let addr = ([127, 0, 0, 1], 3000).into();

    let mk_svc =
        make_service_fn(|_conn| async { Ok::<_, std::convert::Infallible>(service_fn(route)) });

    let srv = Server::bind(&addr).serve(mk_svc);

    if let Err(e) = srv.await {
        eprintln!("server error: {}", e);
    }
}

/// Route the request to the correct handler
///
/// Some routes are built in (like healthz), while others are dynamically
/// dispatched.
async fn route(req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
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
        _ => match find_wasm_module(uri_path) {
            Ok(module) => Ok(module.execute(&req)),
            Err(e) => {
                eprintln!("error: {}", e);
                Ok(not_found())
            }
        },
    }
}

/// Load the configuration TOML and find a module that matches
fn find_wasm_module(uri_path: &str) -> Result<Module, anyhow::Error> {
    let config = load_modules_toml()?;
    let found = config
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

/// Load the configuration TOML
fn load_modules_toml() -> Result<ModuleConfig, anyhow::Error> {
    let data = std::fs::read_to_string("./examples/modules.toml")?;
    let modules: ModuleConfig = toml::from_str(data.as_str())?;
    Ok(modules)
}

/// The configuration for all modules in a WAGI site
#[derive(Clone, Deserialize)]
struct ModuleConfig {
    module: Vec<Module>,
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
    /// Files on the local filesystem that can be opened by this module
    /// Files should be absolute paths. They will be pre-opened immediately before the
    /// they are loaded into the WASM module.
    pub files: Option<Vec<String>>,
}

impl Module {
    /// Execute the WASM module in a WAGI
    fn execute(&self, req: &Request<Body>) -> Response<Body> {
        match self.run_wasm(req) {
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

    // Load and execute the WASM module.
    //
    // Typically, the higher-level execute() method should be used instead, as that handles
    // wrapping errors in the appropriate HTTP response. This is a lower-level function
    // that returns the errors that occur during processing of a WASM module.
    //
    // Note that on occasion, this module COULD return an Ok() with a response body that
    // contains an HTTP error. This can occur, for example, if the WASM module sets
    // the status code on its own.
    fn run_wasm(&self, req: &Request<Body>) -> Result<Response<Body>, anyhow::Error> {
        let store = Store::default();
        let mut linker = Linker::new(&store);

        // Normalize headers
        let mut headers = std::collections::HashMap::new();
        req.headers().iter().for_each(|header| {
            // TODO: Need to figure out what a Hyper HeaderValue is
            // TODO: Normalize headers into env vars, following rules in CGI spec
            //let key = header.0.to_string().to_upper();
            headers.insert(header.0.to_string(), "some value".to_owned());
        });

        // TODO: STDIN should be attached to the Request's Body
        //let stdin = std::io::stdin();

        // TODO: STDOUT should be attached to something that eventually produces they hyper::Body
        //let stdout = std::io::stdout();

        // TODO: The spec does not say what to do with STDERR. Currently, we will attach
        // to wherever logs go.

        // TODO: Add support for Module.file to preopen.

        let ctx = WasiCtxBuilder::new()
            .args(vec![req.uri().path()]) // TODO: Query params go in args. Read spec.
            .envs(headers)
            .inherit_stdio() // TODO: this should be replaced
            .build()?;
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

        // TODO: So technically a CGI gateway processor MUST parse the resulting headers
        // and rewrite some (while removing others). This should be fairly trivial to do,
        // and it might even be possible to do with the `h2` library, which may have a method
        // for parsing the raw body of an HTTP request.
        //
        // The headers should then be added to the response headers, and the body should
        // be passed back untouched.

        Ok(Response::new(Body::from(format!(
            "executed module {}",
            self.module
        ))))
    }
}

/// Create an HTTP 404 response
fn not_found() -> Response<Body> {
    let mut not_found = Response::default();
    *not_found.status_mut() = StatusCode::NOT_FOUND;
    not_found
}
