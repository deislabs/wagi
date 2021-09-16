use std::{collections::HashMap, sync::Arc};

use bindle::Invoice;
use tracing::instrument;

use crate::bindle_util::InterestingParcel;
use crate::wagi_config::{HandlerConfiguration, ModuleMapConfiguration, ModuleMapConfigurationEntry};
use crate::wasm_module::WasmModuleSource;

pub struct RoutingTable {
    pub entries: Vec<RoutingTableEntry>,
}

#[derive(Clone)]
pub struct RoutingTableEntry {
    pub route_pattern: RoutePattern,
    pub handler_info: RouteHandler,
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
pub enum RouteHandler {
    HealthCheck,
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

const DEFAULT_ENTRYPOINT: &str = "_start";

const FAKE_WASM_FAKE_FAKE_FAKE: Vec<u8> = vec![];
fn FAKE_VOLUMES_FAKE_FAKE_FAKE() -> HashMap<String, String> { HashMap::new() }

impl RoutingTableEntry {
    pub fn is_match(&self, uri_fragment: &str) -> bool {
        self.route_pattern.is_match(uri_fragment)
    }

    fn build_from_modules_toml(source: &ModuleMapConfigurationEntry) -> anyhow::Result<RoutingTableEntry> {
        let route_pattern = RoutePattern::parse(&source.route);
        let handler_info = RouteHandler::Wasm(WasmModuleSource::Blob(Arc::new(FAKE_WASM_FAKE_FAKE_FAKE)));

        Ok(Self {
            route_pattern,
            handler_info,
            entrypoint: source.entrypoint.clone().unwrap_or_else(|| DEFAULT_ENTRYPOINT.to_owned()),
            volumes: FAKE_VOLUMES_FAKE_FAKE_FAKE(),  // TODO
            allowed_hosts: source.allowed_hosts.clone().unwrap_or_else(|| vec![]),
            http_max_concurrency: source.http_max_concurrency,
        })
    }

    fn build_from_parcel(source: &InterestingParcel) -> Option<anyhow::Result<RoutingTableEntry>> {
        match source {
            InterestingParcel::WagiHandler(wagi_handler) => {
                let route_pattern = RoutePattern::parse(&wagi_handler.route);
                let handler_info = RouteHandler::Wasm(WasmModuleSource::Blob(Arc::new(FAKE_WASM_FAKE_FAKE_FAKE)));

                Some(Ok(Self {
                    route_pattern,
                    handler_info,
                    entrypoint: wagi_handler.entrypoint.clone().unwrap_or_else(|| DEFAULT_ENTRYPOINT.to_owned()),
                    volumes: FAKE_VOLUMES_FAKE_FAKE_FAKE(),  // TODO
                    allowed_hosts: wagi_handler.allowed_hosts.clone(),
                    http_max_concurrency: None,
                }))
            },
        }
    }

    fn inbuilt(path: &str, handler: RouteHandler) -> Self {
        Self {
            route_pattern: RoutePattern::Exact(path.to_owned()),
            handler_info: handler,
            // TODO: maybe these should be on the RouteHandler Wasm case
            entrypoint: Default::default(),
            volumes: Default::default(),
            allowed_hosts: Default::default(),
            http_max_concurrency: Default::default(),
        }
    }
}

impl RoutePattern {
    fn parse(path_text: &str) -> Self {
        match path_text.strip_suffix("/...") {
            Some(prefix) => Self::Prefix(prefix.to_owned()),
            None => Self::Exact(path_text.to_owned())
        }
    }

    pub fn is_match(&self, uri_fragment: &str) -> bool {
        match self {
            Self::Exact(path) => path == uri_fragment,
            Self::Prefix(prefix) => uri_fragment.starts_with(prefix),
        }
    }
}

impl RoutingTable {
    pub fn build(source: &HandlerConfiguration) -> anyhow::Result<RoutingTable> {
        let user_entries = match source {
            HandlerConfiguration::ModuleMapFile(module_map_configuration) =>
                Self::build_from_modules_toml(module_map_configuration),
            HandlerConfiguration::Bindle(invoice) =>
                Self::build_from_bindle(invoice),
        }?;
        let built_in_entries = Self::inbuilt_patterns();

        let entries = user_entries.into_iter().chain(built_in_entries).collect();
        Ok(Self { entries })
    }

    fn build_from_modules_toml(module_map_configuration: &ModuleMapConfiguration) -> anyhow::Result<Vec<RoutingTableEntry>> {
        // TODO: look for `_routes` function
        module_map_configuration.entries
            .iter()
            .map(|e| RoutingTableEntry::build_from_modules_toml(e))
            .collect()
    }

    fn build_from_bindle(invoice: &Invoice) -> anyhow::Result<Vec<RoutingTableEntry>> {
        // TODO: this is duplication!  We should parse the invoice once into a structure of relevant stuff
        let top = crate::bindle_util::top_modules(invoice);
        let interesting_parcels = top.iter().filter_map(|p| crate::bindle_util::classify_parcel(p));

        interesting_parcels
            .filter_map(|p| RoutingTableEntry::build_from_parcel(&p)).collect()
    }

    fn inbuilt_patterns() -> Vec<RoutingTableEntry> {
        vec![
            RoutingTableEntry::inbuilt("/healthz", RouteHandler::HealthCheck),
        ]
    }
}
