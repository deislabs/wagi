use crate::{
    bindle_util::BindleConnectionInfo,
    wagi_config::{
        HandlerConfigurationSource, HttpConfiguration, TlsConfiguration, WagiConfiguration,
    },
};
use clap::{App, Arg, ArgGroup, ArgMatches};
use core::convert::TryFrom;
use std::collections::HashMap;
use std::net::SocketAddr;

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
const ARG_BINDLE_INSECURE: &str = "bindle_insecure";
const ARG_BINDLE_HTTP_USER: &str = "BINDLE_HTTP_USER";
const ARG_BINDLE_HTTP_PASSWORD: &str = "BINDLE_HTTP_PASSWORD";

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

// Groups
const GROUP_MODULE_SOURCE: &str = "module_source";
const GROUP_BINDLE_SOURCE: &str = "bindle_source";

pub fn wagi_app_definition() -> App<'static, 'static> {
    App::new("WAGI Server")
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
            .value_name("BINDLE_ID")
            .help("A bindle ID, such as foo/bar/1.2.3")
            .takes_value(true)
            .requires(GROUP_BINDLE_SOURCE),
    )
    .group(
        ArgGroup::with_name(GROUP_MODULE_SOURCE)
            .args(&[ARG_MODULES_CONFIG, ARG_BINDLE_ID])
            .required(true)
    )
    .arg(
        Arg::with_name(ARG_BINDLE_STANDALONE_DIR)
            .long("bindle-path")
            .help("A base path for standalone bindles")
            .takes_value(true)
            .requires(ARG_BINDLE_ID)
    )
    .arg(
        Arg::with_name(ARG_BINDLE_URL)
            .long("bindle-url")
            .value_name(BINDLE_URL)
            .env(BINDLE_URL)
            .help("The Bindle server URL, e.g. https://example.com:8080/v1. Note that the version path (v1) is required.")
            .takes_value(true)
    )
    .group(
        ArgGroup::with_name(GROUP_BINDLE_SOURCE)
            .args(&[ARG_BINDLE_STANDALONE_DIR, ARG_BINDLE_URL])
    )
    .arg(
        Arg::with_name(ARG_BINDLE_HTTP_USER)
            .long("bindle-http-user")
            .value_name("BINDLE_HTTP_USER")
            .env("BINDLE_HTTP_USER")
            .help("The username for authentication via basic http auth with the Bindle server.")
            .takes_value(true)
            .requires(ARG_BINDLE_HTTP_PASSWORD)
    )
    .arg(
        Arg::with_name(ARG_BINDLE_HTTP_PASSWORD)
            .long("bindle-http-password")
            .value_name("BINDLE_HTTP_PASSWORD")
            .env("BINDLE_HTTP_PASSWORD")
            .help("The password for authentication via basic http auth with the Bindle server.")
            .takes_value(true)
            .requires(ARG_BINDLE_HTTP_USER)
    )
    .arg(
        Arg::with_name(ARG_BINDLE_INSECURE)
            .short("k")
            .long("bindle-insecure")
            .help("if set, ignore bindle server certificate errors")
            .required(false)
            .takes_value(false),
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
    .arg(
        Arg::with_name(ARG_ENV_FILES)
            .long("env-file")
            .takes_value(true)
            .value_name("ENV_FILE")
            .multiple(true)
            .help("Read a file of NAME=VALUE pairs and parse it into environment variables for the guest module. Multiple files can be specified. See also '--env'.")
    )
}

pub fn parse_command_line() -> anyhow::Result<WagiConfiguration> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let wagi_app = wagi_app_definition();

    let matches = wagi_app.get_matches();
    parse_configuration_from(matches)
}

pub fn parse_configuration_from(matches: ArgMatches) -> anyhow::Result<WagiConfiguration> {
    let addr: SocketAddr = matches
        .value_of(ARG_LISTEN_ON)
        .unwrap_or("127.0.0.1:3000")
        .parse()
        .unwrap();

    tracing::info!(?addr, "Starting server");

    // We have to pass a cache file configuration path to a Wasmtime engine.
    let cache_config_path = matches
        .value_of(ARG_WASM_CACHE_CONFIG_FILE)
        .unwrap_or("cache.toml")
        .to_owned();

    let hostname = matches
        .value_of(ARG_DEFAULT_HOSTNAME)
        .unwrap_or("localhost:3000");

    // TODO: this means that we effectively default to no caching between
    // runs - this seems non-optimal
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
        asset_cache_dir: mc,
        log_dir,
    };

    Ok(configuration)
}

fn parse_bindle_connection_info(
    url: url::Url,
    matches: &ArgMatches,
) -> anyhow::Result<BindleConnectionInfo> {
    Ok(BindleConnectionInfo::new(
        url,
        matches.is_present(ARG_BINDLE_INSECURE),
        matches
            .value_of(ARG_BINDLE_HTTP_USER)
            .map(|s| s.to_string()),
        matches
            .value_of(ARG_BINDLE_HTTP_PASSWORD)
            .map(|s| s.to_string()),
    ))
}

fn parse_handler_configuration_source(
    matches: &ArgMatches,
) -> anyhow::Result<HandlerConfigurationSource> {
    // The following rules are enforced at the clap app/arg level:
    //
    // * You MUST have a modules file OR a bindle ID, but not both
    // * If you have a bindle ID (i.e. do NOT have a modules file), you MUST
    //   have a Bindle server URL OR standalone directory, but not both
    match (
        matches.value_of(ARG_BINDLE_ID).ignore_if_empty(),
        matches
            .value_of(ARG_BINDLE_STANDALONE_DIR)
            .ignore_if_empty(),
        matches.value_of(ARG_BINDLE_URL).ignore_if_empty(),
        matches.value_of(ARG_MODULES_CONFIG).ignore_if_empty(),
    ) {
        // Case: got a module file. Can't have bindle id; ignore bindle location.
        (None, _, _, Some(modules_config)) => {
            let modules_config_path = std::path::PathBuf::from(modules_config);
            if modules_config_path.is_file() {
                Ok(HandlerConfigurationSource::ModuleConfigFile(
                    modules_config_path,
                ))
            } else {
                Err(anyhow::anyhow!(
                    "Module file {} does not exist or is not a file",
                    modules_config
                ))
            }
        }
        // Case: got a bindle id and directory. Can't have a server URL or module file.
        (Some(bindle_id), Some(bindle_dir), None, None) => {
            let bindle_dir_path = std::path::PathBuf::from(bindle_dir);
            if bindle_dir_path.is_dir() {
                Ok(HandlerConfigurationSource::StandaloneBindle(
                    bindle_dir_path,
                    bindle::Id::try_from(bindle_id)?,
                ))
            } else {
                Err(anyhow::anyhow!(
                    "Bindle directory {} does not exist or is not a directory",
                    bindle_dir
                ))
            }
        }
        // Case: got a bindle id and server URL. Can't have a bindir dir or module file.
        (Some(bindle_id), None, Some(bindle_url), None) => match url::Url::parse(bindle_url) {
            Ok(url) => Ok(HandlerConfigurationSource::RemoteBindle(
                parse_bindle_connection_info(url, &matches)?,
                bindle::Id::try_from(bindle_id)?,
            )),
            Err(e) => Err(anyhow::anyhow!("Invalid Bindle server URL: {}", e)),
        },
        // These cases shouldn't be able to happen. We could be optimistic and
        // confident, and assume that means they won't. But we have been
        // programming faaaaaaaaaar too long for that.
        // Case SHOULDN'T HAPPEN: got NEITHER module config file NOR bindle id
        (None, _, _, None) => Err(anyhow::anyhow!(
            "You must specify module config file or bindle ID"
        )),
        // Case SHOULDN'T HAPPEN: got a module config file AND bindle id
        (Some(_), _, _, Some(_)) => Err(anyhow::anyhow!(
            "You cannot specify both module config file and bindle ID"
        )),
        // Case SHOULDN'T HAPPEN: got a bindle id and NEITHER directory NOR URL
        (Some(_), None, None, _) => Err(anyhow::anyhow!(
            "A bindle ID requires either a server URL or standalone directory"
        )),
        // Case SHOULDN'T HAPPEN: got a bindle id and BOTH directory AND URL
        (Some(_), Some(_), Some(_), _) => Err(anyhow::anyhow!(
            "You cannot specify both a bindle server URL and a standalone directory"
        )),
    }
}

fn parse_tls_config(
    tls_cert_file: Option<&str>,
    tls_key_file: Option<&str>,
) -> anyhow::Result<Option<TlsConfiguration>> {
    match (tls_cert_file, tls_key_file) {
        (Some(cert), Some(key)) => {
            let cert_path = std::path::PathBuf::from(cert);
            let key_path = std::path::PathBuf::from(key);
            if !cert_path.is_file() {
                Err(anyhow::anyhow!(
                    "TLS certificate file does not exist or is not a file"
                ))
            } else if !key_path.is_file() {
                Err(anyhow::anyhow!(
                    "TLS key file does not exist or is not a file"
                ))
            } else {
                Ok(Some(TlsConfiguration {
                    cert_path,
                    key_path,
                }))
            }
        }
        (None, None) => Ok(None),
        // Should be impossible from arg requirements
        _ => Err(anyhow::anyhow!(
            "Both a cert and key file should be set or neither should be set"
        )),
    }
}

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

trait EmptyIgnorer {
    fn ignore_if_empty(&self) -> Self;
}

impl EmptyIgnorer for Option<&str> {
    fn ignore_if_empty(&self) -> Self {
        match self {
            Some("") => None,
            None => None,
            Some(s) => Some(s),
        }
    }
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
