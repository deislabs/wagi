use core::convert::TryFrom;
use std::sync::Arc;

use bindle::Parcel;

use crate::{caching_bindle_client::CachingBindleClient, wagi_config::{HandlerConfigurationSource, ModuleMapConfigurationEntry, WagiConfiguration}};

pub async fn load_from_module_map_entry(module_map_entry: &ModuleMapConfigurationEntry, configuration: &WagiConfiguration) -> anyhow::Result<Vec<u8>> {
    // TODO: code far too similar to required blobs stuff
    let module_ref = module_map_entry.module.clone();
    match url::Url::parse(&module_ref) {
        Err(e) => {
            tracing::debug!(
                error = %e,
                "Error parsing module URI. Assuming this is a local file"
            );
            Ok(tokio::fs::read(&module_ref).await?)
        },
        Ok(uri) => match uri.scheme() {
            "file" => match uri.to_file_path() {
                Ok(p) => Ok(tokio::fs::read(p).await?),  // TODO: include path in error
                Err(e) => Err(anyhow::anyhow!("Cannot get path to file {}: {:#?}", module_ref, e)),
            }
            "bindle" => {
                // TODO: should we allow --bindle-server so modules.toml can resolve?  This is deprecated so not keen
                let bindle_server = module_map_entry.bindle_server.as_ref().ok_or_else(|| anyhow::anyhow!("No Bindle server specified for module {}", module_ref))?;
                let _bindle_id = bindle::Id::try_from(uri.path())?;
                let _bindle_client = bindle::client::Client::new(bindle_server)?;
                Err(anyhow::anyhow!("not sure which parcel to get from bindle"))
                // TODO: Ok(bindle_client.get_parcel(&bindle_id, what).await?)
            },
            // "parcel" => self.load_parcel(&uri, store.engine(), cache).await,  // TODO: this is not mentioned in the spec...?
            "oci" => /* TODO: copy stuff */ Err(anyhow::anyhow!("we don't do OCI yet")),
            s => Err(anyhow::anyhow!("Unknown scheme {} in module reference {}", s, module_ref)),
        }
    }
}

pub async fn load_from_bindle(invoice_id: &bindle::Id, parcel: &Parcel, configuration: &WagiConfiguration) -> anyhow::Result<Vec<u8>> {
    match &configuration.handlers {
        HandlerConfigurationSource::ModuleConfigFile(_) => panic!("load_from_bindle called when modules.toml config specified"),
        HandlerConfigurationSource::StandaloneBindle(base_path, _) => {
            let reader = bindle::standalone::StandaloneRead::new(&base_path, invoice_id).await?;
            let mpath = reader.parcel_dir
                .join(format!("{}.dat", parcel.label.sha256))
                .to_string_lossy()
                .to_string();
            let parcel_bytes = tokio::fs::read(mpath).await?;
            Ok(parcel_bytes)
        },
        HandlerConfigurationSource::RemoteBindle(server_url, _) => {
            let client = CachingBindleClient::new(server_url, &configuration.asset_cache_dir)?;
            let parcel_bytes = client.get_module_parcel(invoice_id, &parcel).await?;
            Ok(parcel_bytes)
        },
    }
}

pub struct Loaded<T> {
    pub metadata: T,
    pub content: Arc<Vec<u8>>,
}

impl<T: Clone> Loaded<T> {
    pub fn new(metadata: &T, content: Vec<u8>) -> Self {
        Self {
            metadata: metadata.clone(),
            content: Arc::new(content),
        }
    }
}
