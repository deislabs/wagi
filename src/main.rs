use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use serde::Deserialize;

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
    let uri_path = req.uri().path();
    match uri_path {
        "/healthz" => Ok(Response::new(Body::from("OK"))),
        _ => match find_wasm_module(uri_path) {
            Ok(module) => Ok(module.execute()),
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
    let found = config.module.iter().filter(|m| m.route == uri_path).last();
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
struct Module {
    route: String,
    module: String,
}

impl Module {
    /// Execute the WASM module in a WAGI
    fn execute(&self) -> Response<Body> {
        Response::new(Body::from(format!("module: {}", self.module)))
    }
}

/// Create an HTTP 404 response
fn not_found() -> Response<Body> {
    let mut not_found = Response::default();
    *not_found.status_mut() = StatusCode::NOT_FOUND;
    not_found
}
