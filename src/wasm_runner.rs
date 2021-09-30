use std::path::{Path};
use std::sync::{Arc, RwLock};

use wasi_common::pipe::{ReadPipe, WritePipe};
use wasmtime::*;
use wasmtime_wasi::*;

use crate::request::{RequestGlobalContext};
use crate::wasm_module::WasmModuleSource;

const STDERR_FILE: &str = "module.stderr";

pub fn prepare_stdio_streams(body: Vec<u8>, global_context: &RequestGlobalContext, handler_id: String) -> Result<crate::wasm_module::IORedirectionInfo, Error> {
    let stdin = ReadPipe::from(body);
    let stdout_buf: Vec<u8> = vec![];
    let stdout_mutex = Arc::new(RwLock::new(stdout_buf));
    let stdout = WritePipe::from_shared(stdout_mutex.clone());
    let log_dir = global_context.base_log_dir.join(handler_id);

    // The spec does not say what to do with STDERR.
    // See specifically sections 4.2 and 6.1 of RFC 3875.
    // Currently, we will attach to wherever logs go.
    tracing::info!(log_dir = %log_dir.display(), "Using log dir");
    std::fs::create_dir_all(&log_dir)?;
    let stderr = cap_std::fs::File::from_std(
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(log_dir.join(STDERR_FILE))?,
        ambient_authority(),
    );
    let stderr = wasi_cap_std_sync::file::File::from_cap_std(stderr);

    Ok(crate::wasm_module::IORedirectionInfo {
        streams: crate::wasm_module::IOStreamRedirects {
            stdin,
            stdout,
            stderr,
        },
        stdout_mutex,
    })
}

pub fn new_store_and_engine(
    cache_config_path: &Path,
    ctx: WasiCtx,
) -> Result<(Store<WasiCtx>, Engine), anyhow::Error> {
    let mut config = Config::default();
    if let Ok(p) = std::fs::canonicalize(cache_config_path) {
        config.cache_config_load(p)?;
    };

    let engine = Engine::new(&config)?;
    Ok((Store::new(&engine, ctx), engine))
}

pub fn prepare_wasm_instance(global_context: &RequestGlobalContext, ctx: WasiCtx, wasm_module_source: &WasmModuleSource, customise_linker: impl Fn(&mut Linker<WasiCtx>) -> anyhow::Result<()>) -> Result<(Store<WasiCtx>, Instance), Error> {
    let (mut store, engine) = new_store_and_engine(&global_context.cache_config_path, ctx)?;
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::add_to_linker(&mut linker, |cx| cx)?;

    customise_linker(&mut linker)?;
    
    let module = wasm_module_source.load_module(&store)?;
    let instance = linker.instantiate(&mut store, &module)?;
    Ok((store, instance))
}

pub fn run_prepared_wasm_instance(instance: Instance, mut store: Store<WasiCtx>, entrypoint: &str, wasm_module_name: &str) -> Result<(), Error> {
    let start = instance
        .get_func(&mut store, entrypoint)
        .ok_or_else(|| anyhow::anyhow!("No such function '{}' in {}", entrypoint, wasm_module_name))?;
    tracing::trace!("Calling Wasm entry point");
    start.call(&mut store, &[])?;
    Ok(())
}

pub fn run_prepared_wasm_instance_if_present(instance: Instance, mut store: Store<WasiCtx>, entrypoint: &str) -> RunWasmResult<(), Error> {
    match instance.get_func(&mut store, entrypoint) {
        Some(func) => match func.call(&mut store, &[]) {
            Ok(_) => RunWasmResult::Ok(()),
            Err(e) => RunWasmResult::WasmError(e),
        },
        None => RunWasmResult::EntrypointNotFound,
    }
}

pub enum RunWasmResult<T, E> {
    Ok(T),
    WasmError(E),
    EntrypointNotFound,
}
