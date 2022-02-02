use std::{fmt::Debug, sync::{Arc, RwLock}, path::Path};

use wasi_common::pipe::{ReadPipe, WritePipe};
use wasmtime::*;

// In future this might be pre-instantiated or something like that, so we will
// just abstract it to be safe.
#[derive(Clone)]
pub enum WasmModuleSource {
    Compiled(Module, Engine)
}

impl Debug for WasmModuleSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Compiled(m, _) => f.write_fmt(format_args!("Compiled(Module={:?})", m.name())),
        }
    }
}

impl WasmModuleSource {
    /// Create a new Wasm Engine and configure it.
    fn new_engine(cache_config_path: &Path) -> anyhow::Result<Engine> {
        let mut config = Config::default();

        // Enable multi memory and module linking support.
        config.wasm_multi_memory(true);
        config.wasm_module_linking(true);

        if let Ok(p) = std::fs::canonicalize(cache_config_path) {
            config.cache_config_load(p)?;
        };

        Engine::new(&config)
    }

    pub fn from_module_bytes(data: Arc<Vec<u8>>, cache_config_path: &Path) -> anyhow::Result<WasmModuleSource> {
        let engine = Self::new_engine(cache_config_path)?;
        let module = wasmtime::Module::new(&engine, &**data)?;
        Ok(WasmModuleSource::Compiled(module, engine))
    }

    pub fn get_compiled_module(&self) -> anyhow::Result<(Module, Engine)> {
        match self {
            Self::Compiled(m, e) =>
                Ok((m.clone(), e.clone()))
        }
    }
}

// This is currently separated out because it has different ownership
// constraints from the stdout_mutex. Not sure how to do this better.
// (I don't want to .clone() the fields even though that would work,
// because that is misleading about the semantics.)
pub struct IOStreamRedirects {
    pub stdin: ReadPipe<std::io::Cursor<Vec<u8>>>,
    pub stdout: WritePipe<Vec<u8>>,
    pub stderr: wasi_cap_std_sync::file::File,
}

pub struct IORedirectionInfo {
    pub streams: IOStreamRedirects,
    pub stdout_mutex: Arc<RwLock<Vec<u8>>>,
}
