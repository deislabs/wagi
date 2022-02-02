use std::{collections::HashMap, net::SocketAddr, path::{Path, PathBuf}};

use anyhow::Context;
use bindle::Invoice;
use serde::Deserialize;

use crate::{bindle_util::{InvoiceUnderstander, WagiHandlerInfo}, emplacer::{Emplacer}, module_loader::{Loaded}, request::RequestGlobalContext, handler_compiler::WasmCompilationSettings};
use crate::handler_config::*;

#[derive(Clone, Debug)]
pub struct WagiConfiguration {
    pub handlers: HandlerConfigurationSource,
    pub env_vars: HashMap<String, String>,
    pub http_configuration: HttpConfiguration,
    pub wasm_cache_config_file: PathBuf,
    pub asset_cache_dir: PathBuf,
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

#[allow(clippy::large_enum_variant)]
pub enum HandlerConfiguration {
    ModuleMapFile(ModuleMapConfiguration),
    Bindle(Emplacer, Invoice),
}

pub type LoadedHandlerConfiguration = LoadedHandlerConfigurationImpl<std::sync::Arc<Vec<u8>>>;
pub type WasmHandlerConfiguration = LoadedHandlerConfigurationImpl<crate::wasm_module::WasmModuleSource>;

pub type LoadedHandlerConfigurationEntry = LoadedHandlerConfigurationEntryImpl<std::sync::Arc<Vec<u8>>>;
pub type WasmHandlerConfigurationEntry = LoadedHandlerConfigurationEntryImpl<crate::wasm_module::WasmModuleSource>;

// TODO: consider replacing these functions with Into implementations
impl LoadedHandlerConfigurationEntry {
    fn from_loaded_module_map_entry(lmmce: Loaded<ModuleMapConfigurationEntry>) -> Self {
        let info = HandlerInfo {
            name: lmmce.metadata.module,
            route: lmmce.metadata.route,
            entrypoint: lmmce.metadata.entrypoint,
            allowed_hosts: lmmce.metadata.allowed_hosts,
            http_max_concurrency: lmmce.metadata.http_max_concurrency,
            volume_mounts: lmmce.metadata.volumes.unwrap_or_default(),
        };
        Self {
            info,
            module: lmmce.content,
        }
    }

    fn from_loaded_bindle_handler(whib: (WagiHandlerInfo, crate::emplacer::Bits)) -> Self {
        let (whi, bits) = whib;
        let info = HandlerInfo {
            name: whi.parcel.label.name,
            route: whi.route,
            entrypoint: whi.entrypoint,
            allowed_hosts: whi.allowed_hosts,
            http_max_concurrency: None,
            volume_mounts: bits.volume_mounts,
        };
        Self {
            info,
            module: bits.wasm_module,
        }
    }
}

pub enum PreHandlerConfiguration {
    ModuleMapFile(PathBuf),
    Bindle(Emplacer, Invoice),
}

impl WagiConfiguration {
    // TODO: we might need to do some renaming here to reflect that the source
    // may include non-handler roles in future
    async fn read_handler_configuration(pre_handler_config: PreHandlerConfiguration) -> anyhow::Result<HandlerConfiguration> {
        match pre_handler_config {
            PreHandlerConfiguration::ModuleMapFile(path) =>
                read_module_map_configuration(&path).await.map(HandlerConfiguration::ModuleMapFile),
            PreHandlerConfiguration::Bindle(emplacer, invoice) =>
                Ok(HandlerConfiguration::Bindle(emplacer, invoice)),
        }
    }

    pub async fn load_handler_configuration(&self, pre_handler_config: PreHandlerConfiguration) -> anyhow::Result<LoadedHandlerConfiguration> {
        let handler_configuration_metadata = Self::read_handler_configuration(pre_handler_config).await?;

        match handler_configuration_metadata {
            HandlerConfiguration::ModuleMapFile(module_map_configuration) =>
                handlers_for_module_map(&module_map_configuration, self).await,
            HandlerConfiguration::Bindle(emplacer, invoice) =>
                handlers_for_bindle(&invoice, &emplacer).await,
        }
    }

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

async fn read_module_map_configuration(path: &Path) -> anyhow::Result<ModuleMapConfiguration> {
    tracing::info!(?path, "Loading modules config file");
    if !tokio::fs::metadata(&path)
        .await
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        return Err(anyhow::anyhow!(
            "no modules configuration file found at {}",
            path.display()
        ));
    }

    let data = std::fs::read(path)
        .with_context(|| format!("Couldn't read module config file at {}", path.display()))?;
    let modules: ModuleMapConfiguration = toml::from_slice(&data)
        .with_context(|| format!("File {} contained invalid TOML or was not a WAGI module config", path.display()))?;
    Ok(modules)
}

async fn handlers_for_module_map(module_map: &ModuleMapConfiguration, configuration: &WagiConfiguration) -> anyhow::Result<LoadedHandlerConfiguration> {
    let loaders = module_map
        .entries
        .iter()
        .map(|e| handler_for_module_map_entry(e, configuration));

    let loadeds: anyhow::Result<Vec<_>> = futures::future::join_all(loaders).await.into_iter().collect();
    
    let entries =
        loadeds?
        .into_iter()
        .map(LoadedHandlerConfigurationEntry::from_loaded_module_map_entry)
        .collect();

    Ok(LoadedHandlerConfiguration { entries })
}

async fn handlers_for_bindle(invoice: &bindle::Invoice, emplacer: &Emplacer) -> anyhow::Result<LoadedHandlerConfiguration> {
    let invoice = InvoiceUnderstander::new(invoice);

    let wagi_handlers = invoice.parse_wagi_handlers();

    let loaders = wagi_handlers.iter().map(|h| emplacer.get_bits_for(h));
    let loadeds: anyhow::Result<Vec<_>> = futures::future::join_all(loaders).await.into_iter().collect();

    let entries =
        wagi_handlers
        .into_iter()
        .zip(loadeds?.into_iter())
        .map(LoadedHandlerConfigurationEntry::from_loaded_bindle_handler)
        .collect();

    Ok(LoadedHandlerConfiguration { entries })
}

async fn handler_for_module_map_entry(module_map_entry: &ModuleMapConfigurationEntry, configuration: &WagiConfiguration) -> anyhow::Result<Loaded<ModuleMapConfigurationEntry>> {
    crate::module_loader::load_from_module_map_entry(module_map_entry, configuration)
        .await
        .map(|v| Loaded::new(module_map_entry, v))
}
