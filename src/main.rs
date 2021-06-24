mod tls;

use clap::{App, Arg};
use hyper::{
    server::conn::AddrStream,
    service::{make_service_fn, service_fn},
};
use hyper::{Body, Response, Server};
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;
use wagi::Router;

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

const ENV_VAR_HELP: &str = "specifies an environment variable that should be used for every module WAGI runs. These will override any set by the module config. Multiple environment variables can be set per flag (e.g. -e FOO=bar BAR=baz) or the flag can be used multiple times (e.g. `-e FOO=bar -e BAR=baz`). Variables can be quoted (e.g. FOO=\"my bar\")";

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
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
            Arg::with_name("hostname")
                .long("hostname")
                .value_name("HOSTNAME")
                .takes_value(true)
                .help("the hostname (and the port if not :80) that is to be considered the default. Default: localhost:3000"),
        )
        .arg(
            Arg::with_name("module_cache")
                .long("module-cache")
                .value_name("MODULE_CACHE_DIR")
                .help("the path to a directory where modules can be cached after fetching from remote locations. Default is to create a tempdir.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("log_dir")
                .long("log-dir")
                .value_name("LOG_DIR")
                .help("the path to a directory where module logs should be stored. This directory will have a separate subdirectory created within it per running module. Default is to create a tempdir.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("tls_cert_file")
                .long("tls-cert-file")
                .value_name("TLS_CERT_FILE")
                .takes_value(true)
                .help("the path to the certificate to use for https, if this is not set, normal http will be used. The cert should be in PEM format")
                .requires("tls_key_file")
        )
        .arg(
            Arg::with_name("tls_key_file")
                .long("tls-key-file")
                .value_name("TLS_KEY_FILE")
                .takes_value(true)
                .help("the path to the certificate key to use for https, if this is not set, normal http will be used. The key should be in PKCS#8 format")
                .requires("tls_cert_file")
        )
        .arg(
            Arg::with_name("env_vars")
            .long("env")
            .short("e")
            .value_name("ENV_VARS")
            .help(ENV_VAR_HELP)
            .takes_value(true)
            .multiple(true)
        )
        .get_matches();

    let addr: SocketAddr = matches
        .value_of("listen")
        .unwrap_or("127.0.0.1:3000")
        .parse()
        .unwrap();

    tracing::info!(?addr, "Starting server");

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

    let hostname = matches.value_of("hostname").unwrap_or("localhost:3000");

    let mc = match matches.value_of("module_cache") {
        Some(m) => std::path::PathBuf::from(m),
        None => tempfile::tempdir()?.into_path(),
    };

    let log_dir = match matches.value_of("log_dir") {
        Some(m) => std::path::PathBuf::from(m),
        None => {
            let tempdir = tempfile::tempdir()?;
            println!(
                "No log_dir specified, using temporary directory {} for logs",
                tempdir.path().display()
            );
            tempdir.into_path()
        }
    };

    let env_vars: HashMap<String, String> = match matches.values_of("env_vars") {
        Some(v) => v
            .into_iter()
            .map(parse_env_var)
            .collect::<anyhow::Result<_>>()?,
        None => HashMap::new(),
    };

    let builder = Router::builder()
        .cache_config_path(cache_config_path)
        .module_cache_dir(mc)
        .base_log_dir(log_dir)
        .default_host(hostname)
        .global_env_vars(env_vars);

    let router = match bindle {
        Some(name) => builder.build_from_bindle(name, &bindle_server).await?,
        None => builder.build_from_modules_toml(&module_config_path).await?,
    };

    // NOTE(thomastaylor312): I apologize for the duplicated code here. I tried to work around this
    // by creating a GetRemoteAddr trait, but you can't use an impl Trait in a closure. The return
    // types for the service fns aren't exported and so I couldn't do a wrapper around the router
    // either. This means these services are basically the same, but with different connection types
    match (
        matches.value_of("tls_cert_file"),
        matches.value_of("tls_key_file"),
    ) {
        (Some(cert), Some(key)) => {
            let mk_svc = make_service_fn(move |conn: &TlsStream<TcpStream>| {
                let (inner, _) = conn.get_ref();
                // We are mapping the error because the normal error types are not cloneable and
                // service functions do not like captured vars, even when moved
                let addr_res = inner.peer_addr().map_err(|e| e.to_string());
                let r = router.clone();
                Box::pin(async move {
                    Ok::<_, std::convert::Infallible>(service_fn(move |req| {
                        let r2 = r.clone();
                        // NOTE: There isn't much in the way of error handling we can do here as
                        // this function needs to return an infallible future. Based on the
                        // documentation of the underlying getpeername function
                        // (https://man7.org/linux/man-pages/man2/getpeername.2.html and
                        // https://docs.microsoft.com/en-us/windows/win32/api/winsock/nf-winsock-getpeername)
                        // the only error that will probably occur here is an interrupted connection
                        let a_res = addr_res.clone();
                        async move {
                            match a_res {
                                Ok(addr) => r2.route(req, addr).await,
                                Err(e) => {
                                    tracing::error!(error = %e, "Socket connection error on new connection");
                                    Ok(Response::builder()
                                        .status(hyper::http::StatusCode::INTERNAL_SERVER_ERROR)
                                        .body(Body::from("Socket connection error"))
                                        .unwrap())
                                }
                            }
                        }
                    }))
                })
            });
            Server::builder(tls::TlsHyperAcceptor::new(&addr, cert, key).await?)
                .serve(mk_svc)
                .await?;
        }
        (None, None) => {
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
            Server::bind(&addr).serve(mk_svc).await?;
        }
        // Shouldn't get here, but just in case, print a helpful warning
        _ => anyhow::bail!("Both a cert and key file should be set or neither should be set"),
    }

    Ok(())
}

fn parse_env_var(val: &str) -> anyhow::Result<(String, String)> {
    let (key, value) = val
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("Invalid environment variable, did not find '='"))?;

    // Check if the key is empty (i.e. the user set -e =bar). An environment variable must have a
    // key, but can have an empty value
    if key.is_empty() {
        return Err(anyhow::anyhow!(
            "Environment variable must have a non-empty key"
        ));
    }

    // If the value starts and ends with a double or single quote, assume it is a quoted value and
    // strip the quotes
    let final_value = if value.starts_with('"') && value.ends_with('"') {
        value.trim_matches('"').to_owned()
    } else if value.starts_with('\'') && value.ends_with('\'') {
        value.trim_matches('\'').to_owned()
    } else {
        value.to_owned()
    };

    Ok((key.to_owned(), final_value))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_successful_env_var_parse() {
        parse_env_var("FOO=bar").expect("Normal env var pair should parse");

        parse_env_var("FOO=").expect("No value should parse");

        let (_, value) = parse_env_var("FOO=\"bar -s\"").expect("Double quoted value should parse");
        assert!(
            !value.contains('"'),
            "Double quoted value should have quotes removed"
        );

        let (_, value) = parse_env_var("FOO='bar -s'").expect("Single quoted value should parse");
        assert!(
            !value.contains('\''),
            "Single quoted value should have quotes removed"
        );

        let (_, value) =
            parse_env_var("FOO=\"bar \" -s\"").expect("Non-matching double quote should parse");
        assert!(
            value.match_indices('"').count() == 1,
            "Value with double quote should not have quote removed"
        );

        let (_, value) =
            parse_env_var("FOO=bar\"").expect("Non-matching double quote should parse (case 2)");
        assert!(
            value.match_indices('"').count() == 1,
            "Value with double quote should not have quote removed (case 2)"
        );

        let (_, value) =
            parse_env_var("FOO='bar ' -s'").expect("Non-matching single quote should parse");
        assert!(
            value.match_indices('\'').count() == 1,
            "Value with double quote should not have quote removed"
        );

        let (_, value) = parse_env_var("FOO=\"\"").expect("Empty double quoted value should parse");
        assert!(
            value.is_empty(),
            "Empty double quoted value should be empty"
        );

        let (_, value) = parse_env_var("FOO=''").expect("Empty single quoted value should parse");
        assert!(
            value.is_empty(),
            "Empty single quoted value should be empty"
        );
    }

    #[test]
    fn test_unsuccessful_env_var_parse() {
        parse_env_var("FOO").expect_err("Missing '=' should fail");

        parse_env_var("=bar").expect_err("Missing key should fail");
    }
}
