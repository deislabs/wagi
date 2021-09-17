use std::sync::{Arc, RwLock};

use wasi_common::pipe::{ReadPipe, WritePipe};
use wasmtime::*;
use wasmtime_wasi::*;

// In future this might be pre-instantiated or something like that, so we will
// just abstract it to be safe.
#[derive(Clone)]
pub enum WasmModuleSource {
    Blob(Arc<Vec<u8>>),
}

impl WasmModuleSource {
    pub fn load_module(&self, store: &Store<WasiCtx>) -> anyhow::Result<wasmtime::Module> {
        match self {
            Self::Blob(bytes) => wasmtime::Module::new(store.engine(), &**bytes),
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
