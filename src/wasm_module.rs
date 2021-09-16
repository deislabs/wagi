use std::sync::Arc;

#[derive(Clone)]
pub enum WasmModuleSource {
    Blob(Arc<Vec<u8>>),
}
