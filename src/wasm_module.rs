use std::{fmt::Debug, sync::{Arc, RwLock}};

use wasi_common::pipe::{ReadPipe, WritePipe};
use wasmtime::*;
use wasmtime_wasi::*;

use std::collections::HashMap;
use lazy_static::lazy_static;

// In future this might be pre-instantiated or something like that, so we will
// just abstract it to be safe.
#[derive(Clone)]
pub enum WasmModuleSource {
    Blob(Arc<Vec<u8>>),
}

lazy_static! {
    /// A cache of the modules.
    static ref CACHE: RwLock<HashMap<Vec<u8>, Module>> = RwLock::new(HashMap::new());
}

impl WasmModuleSource {
    pub fn load_module(&self, engine: &Engine) -> anyhow::Result<wasmtime::Module> {
        match self {
            Self::Blob(bytes) => {
                // If we already have the module, return a clone of the cached version.
                // Otherwise we will load the module from bytes, cache the result, and
                // then return the module.
                let data = &**bytes;
                let (wasm_mod, cache_hit) = match CACHE.read().unwrap().get(data) {
                    None => {
                        tracing::debug!("Cache miss. Instantiating new copy");
                        (wasmtime::Module::new(engine, &**bytes)?, false)
                    },
                    Some(m) => {
                        tracing::debug!("Cache hit");
                        (m.clone(), true)
                    },
                };

                if !cache_hit {
                    CACHE.write().unwrap().insert(data.to_vec(), wasm_mod.clone());
                }
                Ok(wasm_mod)
            },
        }
    }
}

impl Debug for WasmModuleSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blob(v) => f.write_fmt(format_args!("Blob(length={})", v.len())),
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
