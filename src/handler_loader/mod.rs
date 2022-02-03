use std::collections::HashMap;

use anyhow::Context;

use crate::{wagi_config::WagiConfiguration, wasm_module::WasmModuleSource};

mod emplacer;
mod loader;
mod compiler;

pub use loader::ModuleMapConfigurationEntry;
pub use compiler::WasmCompilationSettings;

pub async fn load_handlers(configuration: &WagiConfiguration) -> anyhow::Result<WasmHandlerConfiguration> {
    let emplaced_handlers = emplacer::emplace(&configuration /* configuration.handlers, configuration.placement_settings() */).await
        .with_context(|| "Failed to copy modules and assets to local cache")?;
    let loaded_handlers = loader::load(emplaced_handlers, &configuration /* .loader_settings() */).await
        .with_context(|| "Failed to load one or more Wasm modules from source")?;
    let handlers = compiler::compile(loaded_handlers, configuration.wasm_compilation_settings())
        .with_context(|| "Failed to compile one or more Wasm modules")?;
    Ok(handlers)
}

pub struct HandlerInfo {
    pub name: String,
    pub route: String,
    pub entrypoint: Option<String>,
    pub allowed_hosts: Option<Vec<String>>,
    pub http_max_concurrency: Option<u32>,
    pub volume_mounts: HashMap<String, String>,
}

pub struct WasmHandlerConfiguration {
    pub entries: Vec<WasmHandlerConfigurationEntry>,
}

pub struct WasmHandlerConfigurationEntry {
    pub info: HandlerInfo,
    pub module: WasmModuleSource,
}
