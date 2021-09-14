use std::{collections::HashMap, net::SocketAddr, path::PathBuf};

use serde::Deserialize;

#[derive(Clone, Debug)]
pub struct WagiConfiguration {
    pub handlers: HandlerConfigurationSource,
    pub env_vars: HashMap<String, String>,
    pub http_configuration: HttpConfiguration,
    pub wasm_cache_config_file: PathBuf,
    pub remote_module_cache_dir: PathBuf,
    pub log_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub enum HandlerConfigurationSource {
    ModuleConfigFile(PathBuf),
    StandaloneBindle(PathBuf, bindle::Id),
    RemoteBindle(url::Url, bindle::Id),
}

#[derive(Clone, Debug)]
pub struct HttpConfiguration {
    pub listen_on: SocketAddr,
    pub default_hostname: String,
    pub tls: Option<TlsConfiguration>,
}

#[derive(Clone, Debug)]
pub struct TlsConfiguration {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModuleMapConfiguration {
    #[serde(rename = "module")]
    pub entries: Vec<ModuleMapConfigurationEntry>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModuleMapConfigurationEntry {
    // The route to wire up
    pub route: String,
    // The Wasm to wire it up to
    pub module: String,  // file path, file://foo URL, bindle:foo/bar/1.2.3 or oci:foo/bar:1.2.3 (bindle: is deprecated which is good because it's not clear which parcel you'd use)
    pub entrypoint: Option<String>,
    pub bindle_server: Option<String>,
    // The environment in which to run it
    pub volumes: Option<HashMap<String, String>>,
    pub allowed_hosts: Option<Vec<String>>,
    pub http_max_concurrency: Option<u32>,
}
