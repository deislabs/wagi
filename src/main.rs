use clap::{App, Arg};
use hyper::{
    server::conn::AddrStream,
    service::{make_service_fn, service_fn},
};
use hyper::{Body, Request, Response, Server};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use wagi::Router;

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    let matches = App::new("WAGI Server")
        .version("0.1.0")
        .author("DeisLabs")
        .about("Run an HTTP WAGI server")
        .arg(
            Arg::with_name("config")
                .short("c")
                .long("config")
                .value_name("MODULES_TOML")
                .help("the path to the modules.toml configuration file")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("cache")
                .long("cache")
                .value_name("CACHE_TOML")
                .help("the path to the cache.toml configuration file")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("listen")
                .short("l")
                .long("listen")
                .value_name("IP_PORT")
                .takes_value(true)
                .help("the IP address and port to listen on. Default: 127.0.0.1:3000"),
        )
        .get_matches();

    println!("=> Starting server");
    let addr = matches
        .value_of("listen")
        .unwrap_or("127.0.0.1:3000")
        .parse()
        .unwrap();

    // We have to pass a cache file configuration path to a Wasmtime engine.
    let cache_config_path = String::from(matches.value_of("cache").unwrap_or("cache.toml"));

    // We only parse the config file once, then we share it across all threads on the
    // service. This is faster than reloading the config on each request, but it does
    // mean the server has to be restarted to reload config.
    let config = Arc::new(Mutex::new(wagi::load_modules_toml(
        matches.value_of("config").unwrap_or("modules.toml"),
        cache_config_path.clone(),
    )?));

    let mk_svc = make_service_fn(move |conn: &AddrStream| {
        let config = config.clone();
        let addr = conn.remote_addr();
        let cache_config_path = cache_config_path.clone();
        async move {
            Ok::<_, std::convert::Infallible>(service_fn(move |req| {
                let modules_toml = config.lock().unwrap();
                route(
                    req,
                    modules_toml.clone(),
                    cache_config_path.clone(),
                    addr.clone(),
                )
            }))
        }
    });

    let srv = Server::bind(&addr).serve(mk_svc);

    if let Err(e) = srv.await {
        eprintln!("server error: {}", e);
    }
    Ok(())
}

async fn route(
    req: Request<Body>,
    config: wagi::ModuleConfig,
    cache_config_path: String,
    client_addr: SocketAddr,
) -> Result<Response<Body>, hyper::Error> {
    let router = &Router {
        module_config: config,
        cache_config_path,
    };

    router.route(req, client_addr).await
}
