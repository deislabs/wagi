mod tls;

use clap::{App, Arg, ArgMatches};
use core::convert::TryFrom;
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
use wagi::wagi_config::{HandlerConfigurationSource, HttpConfiguration, TlsConfiguration, WagiConfiguration};

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
const BINDLE_URL: &str = "BINDLE_URL";

// Arguments for serving from a bindle
const ARG_BINDLE_ID: &str = "bindle";
const ARG_BINDLE_URL: &str = "BINDLE_URL";
const ARG_BINDLE_STANDALONE_DIR: &str = "bindle_path";

// Arguments for serving from local Wasm files specified in a modules.toml
const ARG_MODULES_CONFIG: &str = "config";

// Wasm execution environment
const ARG_ENV_VARS: &str = "env_vars";
const ARG_ENV_FILES: &str = "env_files";

// HTTP configuration
const ARG_LISTEN_ON: &str = "listen";
const ARG_DEFAULT_HOSTNAME: &str = "hostname";
const ARG_TLS_CERT_FILE: &str = "tls_cert_file";
const ARG_TLS_KEY_FILE: &str = "tls_key_file";

// Program configuration
const ARG_WASM_CACHE_CONFIG_FILE: &str = "cache";
const ARG_REMOTE_MODULE_CACHE_DIR: &str = "module_cache";
const ARG_LOG_DIR: &str = "log_dir";

#[tokio::main]
pub async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let matches = App::new("WAGI Server")
        .version(clap::crate_version!())
        .author("DeisLabs")
        .about(ABOUT)
        .arg(
            Arg::with_name(ARG_MODULES_CONFIG)
                .short("c")
                .long("config")
                .value_name("MODULES_TOML")
                .help("the path to the modules.toml configuration file")
                .takes_value(true),
        )
        .arg(
            Arg::with_name(ARG_BINDLE_ID)
                .short("b")
                .long("bindle")
                .help("A bindle URL, such as bindle:foo/bar/1.2.3")  // TODO: is the bindle: prefix correct/needed?
                .takes_value(true),
        )
        .arg(
            Arg::with_name(ARG_BINDLE_STANDALONE_DIR)
                .long("bindle-path")
                .help("A base path for standalone bindles")
                .takes_value(true)
                .requires(ARG_BINDLE_ID),
        )
        .arg(
            Arg::with_name(ARG_BINDLE_URL)
                .long("bindle-url")
                .value_name(BINDLE_URL)
                .env(BINDLE_URL)
                .help("The Bindle server URL, e.g. https://example.com:8080/v1. Note that the version path (v1) is required.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name(ARG_WASM_CACHE_CONFIG_FILE)
                .long("cache")
                .value_name("CACHE_TOML")
                .help("the path to the cache.toml configuration file for configuring the Wasm optimization cache")
                .takes_value(true),
        )
        .arg(
            Arg::with_name(ARG_LISTEN_ON)
                .short("l")
                .long("listen")
                .value_name("IP_PORT")
                .takes_value(true)
                .help("the IP address and port to listen on. Default: 127.0.0.1:3000"),
        )
        .arg(
            Arg::with_name(ARG_DEFAULT_HOSTNAME)
                .long("hostname")
                .value_name("HOSTNAME")
                .takes_value(true)
                .help("the hostname (and the port if not :80) that is to be considered the default. Default: localhost:3000"),
        )
        .arg(
            Arg::with_name(ARG_REMOTE_MODULE_CACHE_DIR)
                .long("module-cache")
                .value_name("MODULE_CACHE_DIR")
                .help("the path to a directory where modules can be cached after fetching from remote locations. Default is to create a tempdir.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name(ARG_LOG_DIR)
                .long("log-dir")
                .value_name("LOG_DIR")
                .env("WAGI_LOG_DIR")
                .help("the path to a directory where module logs should be stored. This directory will have a separate subdirectory created within it per running module. Default is to create a tempdir.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name(ARG_TLS_CERT_FILE)
                .long("tls-cert")
                .value_name("TLS_CERT")
                .env("WAGI_TLS_CERT")
                .takes_value(true)
                .help("the path to the certificate to use for https, if this is not set, normal http will be used. The cert should be in PEM format")
                .requires(ARG_TLS_KEY_FILE)
        )
        .arg(
            Arg::with_name(ARG_TLS_KEY_FILE)
                .long("tls-key")
                .value_name("TLS_KEY")
                .env("WAGI_TLS_KEY")
                .takes_value(true)
                .help("the path to the certificate key to use for https, if this is not set, normal http will be used. The key should be in PKCS#8 format")
                .requires(ARG_TLS_CERT_FILE)
        )
        .arg(
            Arg::with_name(ARG_ENV_VARS)
            .long("env")
            .short("e")
            .value_name("ENV_VARS")
            .help(ENV_VAR_HELP)
            .takes_value(true)
            .multiple(true)
        )
        .arg(Arg::with_name(ARG_ENV_FILES)
            .long("env-file")
            .takes_value(true)
            .value_name("ENV_FILE")
            .multiple(true)
            .help("Read a file of NAME=VALUE pairs and parse it into environment variables for the guest module. Multiple files can be specified. See also '--env'.")
        )
        .get_matches();

    let addr: SocketAddr = matches
        .value_of(ARG_LISTEN_ON)
        .unwrap_or("127.0.0.1:3000")
        .parse()
        .unwrap();

    tracing::info!(?addr, "Starting server");

    // We have to pass a cache file configuration path to a Wasmtime engine.
    let cache_config_path = matches.value_of(ARG_WASM_CACHE_CONFIG_FILE).unwrap_or("cache.toml").to_owned();

    let hostname = matches.value_of(ARG_DEFAULT_HOSTNAME).unwrap_or("localhost:3000");

    let mc = match matches.value_of(ARG_REMOTE_MODULE_CACHE_DIR) {
        Some(m) => std::path::PathBuf::from(m),
        None => tempfile::tempdir()?.into_path(),
    };

    let log_dir = match matches.value_of(ARG_LOG_DIR) {
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

    let env_vars = merge_env_vars(&matches)?;

    tracing::debug!(?env_vars, "Env vars are set");

    let tls_cert = matches.value_of(ARG_TLS_CERT_FILE);
    let tls_key = matches.value_of(ARG_TLS_KEY_FILE);

    let handlers = parse_handler_configuration_source(&matches)?;
    let tls_config = parse_tls_config(tls_cert, tls_key)?;

    let configuration = WagiConfiguration {
        handlers,
        env_vars,
        http_configuration: HttpConfiguration {
            listen_on: addr,
            default_hostname: hostname.to_owned(),
            tls: tls_config,
        },
        wasm_cache_config_file: std::path::PathBuf::from(cache_config_path),
        remote_module_cache_dir: mc,
        log_dir,
    };

    let router = Router::from_configuration(&configuration).await?;

    // NOTE(thomastaylor312): I apologize for the duplicated code here. I tried to work around this
    // by creating a GetRemoteAddr trait, but you can't use an impl Trait in a closure. The return
    // types for the service fns aren't exported and so I couldn't do a wrapper around the router
    // either. This means these services are basically the same, but with different connection types
    match (tls_cert, tls_key) {
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

fn parse_handler_configuration_source(matches: &ArgMatches) -> anyhow::Result<HandlerConfigurationSource> {
    // This is slightly stricter than the previous rule. Previously:
    // * If you had a bindle ID:
    //   * If you had a standalone path, we used that.
    //   * Otherwise, use URL or default.
    // * Otherwise, use modules file.
    // This could lead to ambiguous combinations (e.g. ID + path + URL).
    //
    // The new logic is therefore:
    // * If you have a bindle ID:
    //   * If you have a standalone path and NO URL, use the standalone path.
    //   * If you have a standalone path AND a URL, error.
    //   * Otherwise, use the URL or default.
    // * Otherwise:
    //   * If you have a standalone bindle path or URL, error.
    //   * Otherwise, use the modules.toml path or default.
    // NOTE: the new rule potentially makes it harder if e.g. you have a BINDLE_URL env var
    // and you want to ignore it in favour of a standalone file. This should be resolved by looking
    // at sources rather than by allowing confusing combinations though!
    match (
        matches.value_of(ARG_BINDLE_ID),
        matches.value_of(ARG_BINDLE_STANDALONE_DIR),
        matches.value_of(ARG_BINDLE_URL),
        matches.value_of(ARG_MODULES_CONFIG)
    ) {
        (None, None, None, modules_config_opt) => {
            let modules_config = modules_config_opt.unwrap_or("modules.toml");
            let modules_config_path = std::path::PathBuf::from(modules_config);
            if modules_config_path.is_file() {
                Ok(HandlerConfigurationSource::ModuleConfigFile(modules_config_path))
            } else {
                Err(anyhow::anyhow!("Module file {} does not exist or is not a file", modules_config))
            }
        },
        (Some(bindle_id), Some(bindle_dir), None, None) => {
            let bindle_dir_path = std::path::PathBuf::from(bindle_dir);
            if bindle_dir_path.is_dir() {
                Ok(HandlerConfigurationSource::StandaloneBindle(bindle_dir_path, bindle::Id::try_from(bindle_id)?))
            } else {
                Err(anyhow::anyhow!("Bindle directory {} does not exist or is not a directory", bindle_dir))
            }
        },
        (Some(bindle_id), None, bindle_url_opt, None) => {
            let bindle_url = bindle_url_opt.unwrap_or("http://localhost:8080/v1");
            match url::Url::parse(bindle_url) {
                Ok(url) => Ok(HandlerConfigurationSource::RemoteBindle(url, bindle::Id::try_from(bindle_id)?)),
                Err(e) => Err(anyhow::anyhow!("Invalid Bindle server URL: {}", e)),
            }
        },
        _ =>
            Err(anyhow::anyhow!("Specify only module config file OR bindle ID + dir OR bindle ID + optional URL")),
    }
}

fn parse_tls_config(tls_cert_file: Option<&str>, tls_key_file: Option<&str>) -> anyhow::Result<Option<TlsConfiguration>> {
    match (tls_cert_file, tls_key_file) {
        (Some(cert), Some(key)) => {
            let cert_path = std::path::PathBuf::from(cert);
            let key_path = std::path::PathBuf::from(key);
            if !cert_path.is_file() {
                Err(anyhow::anyhow!("TLS certificate file does not exist or is not a file"))
            } else if !key_path.is_file() {
                Err(anyhow::anyhow!("TLS key file does not exist or is not a file"))
            } else {
                Ok(Some(TlsConfiguration {
                    cert_path,
                    key_path,
                }))
            }
        },
        (None, None) => Ok(None),
        // Should be impossible from arg requirements
        _ => Err(anyhow::anyhow!("Both a cert and key file should be set or neither should be set")),
    }}

/// Merge environment variables defined in a file with those defined on the CLI.
fn merge_env_vars(matches: &ArgMatches) -> anyhow::Result<HashMap<String, String>> {
    let mut env_vars: HashMap<String, String> = match matches.values_of(ARG_ENV_FILES) {
        Some(v) => env_file_reader::read_files(&v.into_iter().collect::<Vec<&str>>())?,
        None => HashMap::new(),
    };

    if let Some(v) = matches.values_of(ARG_ENV_VARS) {
        let extras: HashMap<String, String> = v
            .into_iter()
            .map(parse_env_var)
            .collect::<anyhow::Result<_>>()?;
        env_vars.extend(extras);
    }
    Ok(env_vars)
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
    use tokio::fs::write;

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

    #[tokio::test]
    async fn test_env_var_merge() {
        // Make sure that env vars are correctly merged together.
        let td = tempfile::tempdir().expect("created a temp dir");
        let evfile = td.path().join("test.env");

        write(&evfile, "FIRST=1\nSECOND=2\n")
            .await
            .expect("wrote env var file");

        let ev_opt = format!("--env-file={}", evfile.display());

        let matches = App::new("env var test")
        .arg(
            Arg::with_name("env_vars")
            .long("env")
            .short("e")
            .value_name("ENV_VARS")
            .help(ENV_VAR_HELP)
            .takes_value(true)
            .multiple(true)
        )
        .arg(Arg::with_name("env_files")
            .long("env-file")
            .takes_value(true)
            .value_name("ENV_FILE")
            .multiple(true)
            .help("Read a file of NAME=VALUE pairs and parse it into environment variables for the guest module. See also '--env'.")
        )
        .get_matches_from(vec!["wagi", "--env", "SECOND=two", "--env", "THIRD=3", ev_opt.as_str()]);

        let env_vars = merge_env_vars(&matches).expect("env vars parsed");

        assert_eq!(
            &"two".to_owned(),
            env_vars
                .get(&"SECOND".to_owned())
                .expect("Found a value for SECOND"),
        );
        assert_eq!(
            &"1".to_owned(),
            env_vars
                .get(&"FIRST".to_owned())
                .expect("Found a value for FIRST"),
        );
        assert_eq!(
            &"3".to_owned(),
            env_vars
                .get(&"THIRD".to_owned())
                .expect("Found a value for THIRD"),
        );

        assert_eq!(3, env_vars.len());

        drop(td);
    }
}
