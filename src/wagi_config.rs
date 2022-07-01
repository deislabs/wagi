use std::{collections::HashMap, net::SocketAddr, path::PathBuf};

use crate::{
    bindle_util::BindleConnectionInfo, handler_loader::WasmCompilationSettings,
    request::RequestGlobalContext,
};

// TODO: figure out how to re-apply the Debug trait here (and on HandlerConfigurationSource)
// At time of writing, it was removed on account of the bindle::client::tokens::token_manager
// not implementing this trait (see crate::bindle_util::BindleConnectionInfo)
#[derive(Clone)]
pub struct WagiConfiguration {
    pub handlers: HandlerConfigurationSource,
    pub env_vars: HashMap<String, String>,
    pub http_configuration: HttpConfiguration,
    pub wasm_cache_config_file: PathBuf,
    pub asset_cache_dir: PathBuf,
    pub log_dir: PathBuf,
}

#[derive(Clone)]
pub enum HandlerConfigurationSource {
    ModuleConfigFile(PathBuf),
    StandaloneBindle(PathBuf, bindle::Id),
    RemoteBindle(BindleConnectionInfo, bindle::Id),
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

impl WagiConfiguration {
    pub fn request_global_context(&self) -> RequestGlobalContext {
        RequestGlobalContext {
            base_log_dir: self.log_dir.clone(),
            default_host: self.http_configuration.default_hostname.to_owned(),
            use_tls: self.http_configuration.tls.is_some(),
            global_env_vars: self.env_vars.clone(),
        }
    }

    pub fn wasm_compilation_settings(&self) -> WasmCompilationSettings {
        WasmCompilationSettings {
            cache_config_path: self.wasm_cache_config_file.clone(),
        }
    }
}
