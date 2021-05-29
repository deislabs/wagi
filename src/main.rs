use anyhow::anyhow;
use clap::{App, Arg};
use hyper::Server;
use hyper::{
    server::conn::AddrStream,
    service::{make_service_fn, service_fn},
};
use log::debug;
use std::net::SocketAddr;
use wagi::{load_modules_toml, Router};

const ABOUT: &str = r#"
Run an HTTP WAGI server

This starts a Wagi server that either uses a config file or a bindle reference to mount routes.
It starts an HTTP server that listens on incoming requests, and then matches the request
route to a Wasm module.

The server runs for the duration of the process. But modules are executed on-demand
when Wagi handles a request. The module is started with the inbound request, and is
terminated as soon as it has returned the response.

Wagi provides a few ways of speeding up performance. The easiest way is to enable a
cache, which will cause all modules to be preloaded and cached on startup.
"#;

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    env_logger::init();
    let matches = App::new("WAGI Server")
        .version("0.1.0")
        .author("DeisLabs")
        .about(ABOUT)
        .arg(
            Arg::with_name("config")
                .short("c")
                .long("config")
                .value_name("MODULES_TOML")
                .help("the path to the modules.toml configuration file")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("bindle")
                .short("b")
                .long("bindle")
                .help("A bindle URL, such as bindle:foo/bar/1.2.3")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("bindle_server_url")
                .long("bindle-server")
                .value_name("BINDLE_SERVER_URL")
                .help("The Bindle server URL, e.g. https://example.com:8080/v1. Note that the version path (v1) is required.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("cache")
                .long("cache")
                .value_name("CACHE_TOML")
                .help("the path to the cache.toml configuration file for configuring the Wasm optimization cache")
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
        .arg(
            Arg::with_name("module_cache")
                .long("module-cache")
                .value_name("MODULE_CACHE_DIR")
                .help("the path to a directory where modules can be cached after fetching from remote locations. Default is to create a tempdir.")
                .takes_value(true),
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

    let bindle_server = matches
        .value_of("bindle_server_url")
        .unwrap_or("http://localhost:8080/v1")
        .to_owned();
    let bindle = matches.value_of("bindle");

    let mc = match matches.value_of("module_cache") {
        Some(m) => std::path::PathBuf::from(m),
        None => tempfile::tempdir()?.into_path(),
    };
    let module_config = match bindle {
        Some(name) => wagi::load_bindle(
            name,
            bindle_server.as_str(),
            cache_config_path.clone(),
            mc.clone(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load {}: {}", name, e))?,
        None => wagi::load_modules_toml(
            module_config_path.as_str(),
            cache_config_path.clone(),
            mc.clone(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load {}: {}", module_config_path, e))?,
    };
    //debug!("Module Config\n {:#?}", module_config);
    let router = Router::new(module_config, cache_config_path, mc).await?;

    let mk_svc = make_service_fn(move |conn: &AddrStream| {
        let addr = conn.remote_addr();
        let r = router.clone();
        async move {
            Ok::<_, std::convert::Infallible>(service_fn(move |req| {
                let r2 = r.clone();
                async move { r2.route(req, addr).await }
            }))
        }
    });

    let srv = Server::bind(&addr).serve(mk_svc);

    if let Err(e) = srv.await {
        log::error!("server error: {}", e);
    }
    Ok(())
}
