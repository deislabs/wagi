use std::{collections::HashMap, net::SocketAddr, path::PathBuf};

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
