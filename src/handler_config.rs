use anyhow::Context;
use std::collections::HashMap;

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
