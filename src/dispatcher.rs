use std::collections::HashMap;

use tracing::instrument;

use crate::wasm_module::WasmModuleSource;

pub struct RoutingTable {
    pub entries: Vec<RoutingTableEntry>,
}

#[derive(Clone)]
pub struct RoutingTableEntry {
    pub route_pattern: RoutePattern,
    pub handler_info: RouteHandlerInfo,
    pub entrypoint: String,
    pub volumes: HashMap<String, String>,
    pub allowed_hosts: Vec<String>,
    pub http_max_concurrency: Option<u32>,
}

#[derive(Clone, Debug)]
pub enum RoutePattern {
    Exact(String),
    Prefix(String),
}

#[derive(Clone)]
pub enum RouteHandlerInfo {
    Inbuilt,
    Wasm(WasmModuleSource),
}

impl RoutingTable {
    #[instrument(level = "trace", skip(self))]
    pub fn route_for(&self, uri_fragment: &str) -> Result<RoutingTableEntry, anyhow::Error> {
        for r in &self.entries {
            tracing::trace!(path = ?r.route_pattern, uri_fragment, "Trying route path");
            if r.is_match(uri_fragment) {
                return Ok(r.clone());
            }
        }

        Err(anyhow::anyhow!("No handler for path {}", uri_fragment))
    }
}

impl RoutingTableEntry {
    pub fn is_match(&self, uri_fragment: &str) -> bool {
        self.route_pattern.is_match(uri_fragment)
    }
}


impl RoutePattern {
    pub fn is_match(&self, uri_fragment: &str) -> bool {
        match self {
            Self::Exact(path) => path == uri_fragment,
            Self::Prefix(prefix) => uri_fragment.starts_with(prefix),
        }
    }
}
