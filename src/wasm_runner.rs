use std::sync::{Arc, RwLock};

use wasi_common::pipe::{ReadPipe, WritePipe};
use wasmtime::*;
use wasmtime_wasi::*;

use tracing::debug;

use crate::request::RequestGlobalContext;
use crate::wasm_module::WasmModuleSource;

const STDERR_FILE: &str = "module.stderr";

#[derive(Clone, Default)]
pub struct WasmLinkOptions {
    pub http_allowed_hosts: Option<Vec<String>>,
    pub http_max_concurrency: Option<u32>,
}

impl WasmLinkOptions {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn with_http(
        self,
        allowed_hosts: Option<Vec<String>>,
        max_concurrency: Option<u32>,
    ) -> Self {
        let mut result = self.clone();
        result.http_allowed_hosts = allowed_hosts;
        result.http_max_concurrency = max_concurrency;
        result
    }

    pub fn apply_to(&self, linker: &mut Linker<WasiCtx>) -> anyhow::Result<()> {
        let http = wasi_experimental_http_wasmtime::HttpCtx::new(
            self.http_allowed_hosts.clone(),
            self.http_max_concurrency,
        )?;
        http.add_to_linker(linker)?;

        Ok(())
    }
}

pub fn prepare_stdio_streams(
    body: Vec<u8>,
    global_context: &RequestGlobalContext,
    handler_id: String,
) -> Result<crate::wasm_module::IORedirectionInfo, Error> {
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

pub fn new_store(ctx: WasiCtx, engine: &Engine) -> Result<Store<WasiCtx>, anyhow::Error> {
    Ok(Store::new(engine, ctx))
}

pub fn prepare_wasm_instance(
    ctx: WasiCtx,
    wasm_module: &WasmModuleSource,
    link_options: WasmLinkOptions,
) -> Result<(Store<WasiCtx>, Instance), Error> {
    debug!("Cloning module object");
    let (module, engine) = wasm_module.get_compiled_module()?;
    let mut store = new_store(ctx, &engine)?;

    debug!("Configuring linker");
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::add_to_linker(&mut linker, |cx| cx)?;
    link_options.apply_to(&mut linker)?;

    debug!("instantiating module in linker");
    let instance = linker.instantiate(&mut store, &module)?;
    Ok((store, instance))
}

pub fn run_prepared_wasm_instance(
    instance: Instance,
    mut store: Store<WasiCtx>,
    entrypoint: &str,
    wasm_module_name: &str,
) -> Result<(), Error> {
    let start = instance.get_func(&mut store, entrypoint).ok_or_else(|| {
        anyhow::anyhow!("No such function '{}' in {}", entrypoint, wasm_module_name)
    })?;
    tracing::trace!("Calling Wasm entry point");
    start.call(&mut store, &[], &mut vec![])?;
    tracing::trace!("Module execution complete");
    Ok(())
}

pub fn run_prepared_wasm_instance_if_present(
    instance: Instance,
    mut store: Store<WasiCtx>,
    entrypoint: &str,
) -> RunWasmResult<(), Error> {
    match instance.get_func(&mut store, entrypoint) {
        Some(func) => match func.call(&mut store, &[], &mut vec![]) {
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
