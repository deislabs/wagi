use clap::{App, Arg};
use hyper::Server;
use hyper::{
    server::conn::AddrStream,
    service::{make_service_fn, service_fn},
};
use wagi::Router;

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    env_logger::init();
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

    let addr: SocketAddr = matches
        .value_of("listen")
        .unwrap_or("127.0.0.1:3000")
        .parse()
        .unwrap();

    log::info!("=> Starting server on {}", addr.to_string());

    // We have to pass a cache file configuration path to a Wasmtime engine.
    let cache_config_path = matches.value_of("cache").unwrap_or("cache.toml").to_owned();
    let module_config_path = matches
        .value_of("config")
        .unwrap_or("modules.toml")
        .to_owned();

    let router = Router::new(module_config_path, cache_config_path)?;

    let mk_svc = make_service_fn(move |conn: &AddrStream| {
        let addr = conn.remote_addr();
        let r = router.clone();
        async move { Ok::<_, std::convert::Infallible>(service_fn(move |req| r.route(req, addr))) }
    });

    let srv = Server::bind(&addr).serve(mk_svc);

    if let Err(e) = srv.await {
        log::error!("server error: {}", e);
    }
    Ok(())
}

/*
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
*/
