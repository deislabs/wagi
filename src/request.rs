use std::{collections::HashMap, net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug)]
pub struct RequestContext {
    pub client_addr: SocketAddr,
}

#[derive(Clone, Debug)]
pub struct RequestGlobalContext {
    pub base_log_dir: PathBuf,
    pub default_host: String,
    pub use_tls: bool,
    pub global_env_vars: HashMap<String, String>,
}
