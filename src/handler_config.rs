use std::collections::HashMap;

pub struct LoadedHandlerConfigurationImpl<M> {
    pub entries: Vec<LoadedHandlerConfigurationEntryImpl<M>>,
}

pub struct LoadedHandlerConfigurationEntryImpl<M> {
    pub name: String,
    pub route: String,
    pub module: M,
    pub entrypoint: Option<String>,
    pub allowed_hosts: Option<Vec<String>>,
    pub http_max_concurrency: Option<u32>,
    pub volume_mounts: HashMap<String, String>,
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
        // TODO: if this fails, include the name in the error
        Ok(LoadedHandlerConfigurationEntryImpl {
            name: self.name,
            route: self.route,
            module: compile(self.module)?,
            entrypoint: self.entrypoint,
            allowed_hosts: self.allowed_hosts,
            http_max_concurrency: self.http_max_concurrency,
            volume_mounts: self.volume_mounts,
        })
    }
}
