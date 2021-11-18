use std::convert::Infallible;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, RwLock};

use wasi_common::pipe::{ReadPipe, WritePipe};
use wasmtime::*;
use wasmtime_wasi::*;

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
        max_concurrency: Option<u32>)
    -> Self {
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
) -> Result<crate::wasm_module::IORedirectionInfo<Vec<u8>>, Error> {
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

pub fn prepare_stdio_streams_for_http(
    body: Vec<u8>,
    stream_writer: crate::stream_writer::StreamWriter,
    global_context: &RequestGlobalContext,
    handler_id: String,
) -> Result<crate::wasm_module::IORedirectionInfo<crate::stream_writer::StreamWriter>, Error> {
    let stdin = ReadPipe::from(body);
    let stdout_mutex = Arc::new(RwLock::new(stream_writer));
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

pub struct SenderPlusPlus2 {
    sender: futures::channel::mpsc::Sender<Result<Vec<u8>, Infallible>>,
}

impl Write for SenderPlusPlus2 {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        println!("write {} bytes", buf.len());
        let len = buf.len();
        let mut v = Vec::with_capacity(len);
        v.extend_from_slice(buf);
        loop {
            match self.sender.try_send(Ok(v)) {
                Ok(_) => break,
                Err(e) => {
                    println!("err");
                    if e.is_full() {
                        println!("retry time!");
                        v = e.into_inner().unwrap();
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        continue;
                    } else {
                        return Err(std::io::Error::new(std::io::ErrorKind::Other, e));
                    }
                },
            }
        }
        Ok(len)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        println!("FLUSHY FLUSHY");
        Ok(())
    }
}

// pub struct BufflesMcPuffles {
//     pending: Vec<Vec<u8>>,
//     write_completed: bool,
// }

// impl BufflesMcPuffles {
//     pub fn mark_finished(&mut self) {
//         self.write_completed = true;
//     }
// }

// impl Write for BufflesMcPuffles {
//     fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
//         let mut v = vec![];
//         for b in buf { v.push(*b); }  // has to be a better way
//         self.pending.push(v);
//         Ok(buf.len())
//     }

//     fn flush(&mut self) -> std::io::Result<()> {
//         Ok(())
//     }
// }

// impl futures_core::Stream for BufflesMcPuffles {
//     type Item = Vec<u8>;

//     fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
//         let mut pending = self.pending;
//         match self.pending.pop() {
//             None => if self.write_completed {
//                 std::task::Poll::Ready(None)
//             } else {
//                 std::task::Poll::Pending
//             },
//             Some(v) => std::task::Poll::Ready(Some(v)),
//         }
//     }
// }

// fn make_write_pipe_over_sender(sender: hyper::body::Sender) -> Arc<RwLock<SenderPlusPlus>> { // WritePipe<SenderPlusPlus> {
//     let spp = SenderPlusPlus { sender };
//     Arc::new(RwLock::new(spp))
//     // let spp_shared = Arc::new(RwLock::new(spp));
//     // WritePipe::from_shared(spp_shared)
// }

// pub struct SenderPlusPlus {
//     sender: hyper::body::Sender,
// }

// impl std::io::Write for SenderPlusPlus {
//     fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
//         let len = buf.len();
//         let bytes = hyper::body::Bytes::copy_from_slice(buf);
//         // let rt = tokio::runtime::Runtime::new().unwrap();  // TODO
//         // let res = rt.block_on(async move {
//         //     self.sender.send_data(bytes).await
//         // }).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

//         // let jh = tokio::spawn(async move {
//         //     self.sender.send_data(bytes).await
//         // });
//         // tokio::task::spawn_blocking(jh);

//         let content = String::from_utf8_lossy(&bytes);
//         tracing::error!("Response chunk: '{}'", content);

//         self.sender.try_send_data(bytes)
//             .map_err(|b| {
//                 let content = String::from_utf8_lossy(&b);
//                 std::io::Error::new(std::io::ErrorKind::Other, anyhow::anyhow!("Sender could not accept moar bytes '{}'", content))
//             })?;

//         tracing::error!("...sent okay");

//         Ok(len)
//     }

//     fn flush(&mut self) -> std::io::Result<()> {
//         Ok(())  // ?
//     }
// }

pub fn new_store_and_engine(
    cache_config_path: &Path,
    ctx: WasiCtx,
) -> Result<(Store<WasiCtx>, Engine), anyhow::Error> {
    let mut config = Config::default();

    // Enable multi memory and module linking support.
    config.wasm_multi_memory(true);
    config.wasm_module_linking(true);

    if let Ok(p) = std::fs::canonicalize(cache_config_path) {
        config.cache_config_load(p)?;
    };

    let engine = Engine::new(&config)?;
    Ok((Store::new(&engine, ctx), engine))
}

pub fn prepare_wasm_instance(
    global_context: &RequestGlobalContext,
    ctx: WasiCtx,
    wasm_module_source: &WasmModuleSource,
    link_options: WasmLinkOptions,
) -> Result<(Store<WasiCtx>, Instance), Error> {
    let (mut store, engine) = new_store_and_engine(&global_context.cache_config_path, ctx)?;
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::add_to_linker(&mut linker, |cx| cx)?;

    link_options.apply_to(&mut linker)?;

    let module = wasm_module_source.load_module(&store)?;
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
