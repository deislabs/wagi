use std::path::{PathBuf};

use crate::handler_loader::{LoadedHandlerConfiguration, WasmHandlerConfiguration};

pub struct WasmCompilationSettings {
    pub cache_config_path: PathBuf,
}

pub fn compile_all(uncompiled_handlers: LoadedHandlerConfiguration, compilation_settings: WasmCompilationSettings) -> anyhow::Result<WasmHandlerConfiguration> {
    uncompiled_handlers.convert_modules(|module_bytes|
        crate::wasm_module::WasmModuleSource::from_module_bytes(module_bytes, &compilation_settings.cache_config_path)
    )
}