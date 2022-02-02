use std::collections::HashMap;

use anyhow::Context;

use crate::{wagi_config::{WagiConfiguration}, handler_compiler::{WasmCompilationSettings, compile_all}};

mod emplacer;
mod loader;

pub use loader::LoadedHandlerConfiguration;
pub use loader::ModuleMapConfigurationEntry;

pub async fn load_handlers(configuration: &WagiConfiguration) -> anyhow::Result<WasmHandlerConfiguration> {
    let emplaced_handlers = emplacer::emplace(&configuration /* configuration.handlers, configuration.placement_settings() */).await
        .with_context(|| "Failed to copy modules and assets to local cache")?;
    let loaded_handlers = loader::load(emplaced_handlers, &configuration /* .loader_settings() */).await
        .with_context(|| "Failed to load one or more Wasm modules from source")?;
    let handlers = compile(loaded_handlers, configuration.wasm_compilation_settings())
        .with_context(|| "Failed to compile one or more Wasm modules")?;
    Ok(handlers)
}

pub type WasmHandlerConfiguration = LoadedHandlerConfigurationImpl<crate::wasm_module::WasmModuleSource>;
pub type WasmHandlerConfigurationEntry = LoadedHandlerConfigurationEntryImpl<crate::wasm_module::WasmModuleSource>;

pub struct HandlerInfo {
    pub name: String,
    pub route: String,
    pub entrypoint: Option<String>,
    pub allowed_hosts: Option<Vec<String>>,
    pub http_max_concurrency: Option<u32>,
    pub volume_mounts: HashMap<String, String>,
}

pub struct LoadedHandlerConfigurationImpl<M> {
    pub entries: Vec<LoadedHandlerConfigurationEntryImpl<M>>,
}

pub struct LoadedHandlerConfigurationEntryImpl<M> {
    pub info: HandlerInfo,
    pub module: M,
}

impl<M> LoadedHandlerConfigurationImpl<M> {
    pub fn convert_modules<O>(self, compile: impl Fn(M) -> anyhow::Result<O>) -> anyhow::Result<LoadedHandlerConfigurationImpl<O>> {
        let result: anyhow::Result<Vec<LoadedHandlerConfigurationEntryImpl<O>>> =
            self
            .entries
            .into_iter()
            .map(|e| e.convert_module(|m| compile(m)))
            .collect();
        Ok(LoadedHandlerConfigurationImpl { entries: result? })
    }
}

impl<M> LoadedHandlerConfigurationEntryImpl<M> {
    pub fn convert_module<O>(self, compile: impl Fn(M) -> anyhow::Result<O>) -> anyhow::Result<LoadedHandlerConfigurationEntryImpl<O>> {
        let compiled_module = compile(self.module)
            .with_context(|| format!("Error compiling Wasm module {}", &self.info.name))?;
        Ok(LoadedHandlerConfigurationEntryImpl {
            info: self.info,
            module: compiled_module,
        })
    }
}

pub fn compile(loaded_handlers: LoadedHandlerConfiguration, settings: WasmCompilationSettings) -> anyhow::Result<WasmHandlerConfiguration> {
    compile_all(loaded_handlers, settings)
}

// // TODO: we might need to do some renaming here to reflect that the source
// // may include non-handler roles in future
// async fn read_handler_configuration(pre_handler_config: PreHandlerConfiguration) -> anyhow::Result<HandlerConfiguration> {
//     match pre_handler_config {
//         PreHandlerConfiguration::ModuleMapFile(path) =>
//             read_module_map_configuration(&path).await.map(HandlerConfiguration::ModuleMapFile),
//         PreHandlerConfiguration::Bindle(emplacer, invoice) =>
//             Ok(HandlerConfiguration::Bindle(emplacer, invoice)),
//     }
// }
