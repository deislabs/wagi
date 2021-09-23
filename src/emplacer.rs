use std::{collections::HashMap, path::{Path, PathBuf}, sync::Arc};

use anyhow::Context;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{bindle_util::{InterestingParcel, InvoiceUnderstander, WagiHandlerInfo}, wagi_config::{HandlerConfigurationSource, WagiConfiguration}};

pub struct Emplacer {
    cache_path: PathBuf,
    source: HandlerConfigurationSource,
}

// pub enum EmplacementSource {
//     ModuleMap,
//     StandaloneBindle(PathBuf, bindle::Id),
//     RemoteBindle(Url, bindle::Id),
// }

pub struct Bits {
    pub wasm_module: Arc<Vec<u8>>,
    pub volume_mounts: HashMap<String, String>,
}

impl Emplacer {
    pub async fn new(configuration: &WagiConfiguration) -> anyhow::Result<Self> {
        let cache_path = configuration.asset_cache_dir.clone();
        tokio::fs::create_dir_all(&cache_path).await
            .with_context(|| format!("Can't create asset cache directory {}", cache_path.display()))?;
        Ok(Self {
            cache_path,
            source: configuration.handlers.clone(),
        })
    }

    pub async fn emplace_all(&self) -> anyhow::Result<()> {
        match &self.source {
            HandlerConfigurationSource::ModuleConfigFile(_) => Ok(()),
            HandlerConfigurationSource::StandaloneBindle(bindle_base_dir, id) =>
                self.emplace_standalone_bindle(bindle_base_dir, id).await,
            HandlerConfigurationSource::RemoteBindle(bindle_base_url, id) =>
                self.emplace_remote_bindle(&bindle_base_url, id).await,
        }.with_context(|| "Error caching assets from bindle")
    }

    // TODO: NO! NO! NO!
    pub async fn get_bits_for(&self, handler: &WagiHandlerInfo) -> anyhow::Result<Bits> {
        let module_parcel_path = self.module_parcel_path(&handler.parcel);
        let wasm_module = tokio::fs::read(&module_parcel_path).await
            .with_context(|| format!("Error reading module {} from cache path {}", handler.parcel.label.name, module_parcel_path.display()))?;
        let volume_mounts = self.asset_dir_volume_mount(&handler.invoice_id);
        Ok(Bits {
            wasm_module: Arc::new(wasm_module),
            volume_mounts,
        })
    }

    // TODO: do not like having bindle specifics here
    pub async fn read_invoice(&self, invoice_id: &bindle::Id) -> anyhow::Result<bindle::Invoice> {
        let toml_text = tokio::fs::read(self.invoice_path(invoice_id)).await?;
        let invoice = toml::from_slice(&toml_text)?;
        Ok(invoice)
    }

    async fn emplace_standalone_bindle(&self, _bindle_base_dir: &PathBuf, _id: &bindle::Id) -> anyhow::Result<()> {
        // let reader = bindle::standalone::StandaloneRead::new(bindle_base_dir, id).await?;
        // reader.
        todo!("YOU HAVEN'T DONE THIS BIT YET TOWLSON!");
    }

    async fn emplace_remote_bindle(&self, bindle_base_url: &Url, id: &bindle::Id) -> anyhow::Result<()> {
        let client = bindle::client::Client::new(bindle_base_url.as_str())?;

        let invoice_path = self.invoice_path(id);
        let invoice = if invoice_path.is_file() {
            // TODO: we should recover from this by going back to the bindle server
            let invoice_text = tokio::fs::read(&invoice_path).await
                .with_context(|| format!("Error reading cached invoice file {}", invoice_path.display()))?;
            toml::from_slice(&invoice_text)
                .with_context(|| format!("Error parsing cached invoice file {}", invoice_path.display()))?
        } else {
            let invoice = client.get_invoice(id).await
                .with_context(|| format!("Error fetching remote invoice {}", &id))?;
            let invoice_text = toml::to_vec(&invoice)
                .with_context(|| format!("Error reserialising remote invoice {} to cache", &id))?;
            safely_write(&invoice_path, invoice_text).await
                .with_context(|| format!("Error writing remote invoice {} to cache", &id))?;
            invoice
        };

        let invoice = InvoiceUnderstander::new(&invoice);

        let module_parcels: Vec<_> = invoice
            .top_modules().iter()
            .filter_map(|parcel| invoice.classify_parcel(parcel))
            .filter_map(|parcel| match parcel {
                InterestingParcel::WagiHandler(h) => Some(h),
            })
            .collect();

        let module_placements = module_parcels.iter().map(|h| self.emplace_module_and_assets(&client, &id, &h));
        let all_module_placements = futures::future::join_all(module_placements).await;

        match all_module_placements.into_iter().find_map(|e| e.err()) {
            Some(e) => Err(e),
            None => Ok(())
        }
    }

    async fn emplace_module_and_assets(&self, client: &bindle::client::Client, invoice_id: &bindle::Id, handler: &WagiHandlerInfo) -> anyhow::Result<()> {
        self.emplace_module(client, invoice_id, &handler.parcel).await?;
        self.emplace_as_assets(client, invoice_id, &handler.asset_parcels()).await?;
        Ok(())
    }

    async fn emplace_module(&self, client: &bindle::client::Client, invoice_id: &bindle::Id, parcel: &bindle::Parcel) -> anyhow::Result<()> {
        let parcel_path = self.cache_path.join(&parcel.label.sha256);
        if parcel_path.is_file() {
            return Ok(());
        }

        let parcel_data = client.get_parcel(invoice_id, &parcel.label.sha256).await
            .with_context(|| format!("Error fetching remote parcel {}", parcel.label.name))?;
        safely_write(&parcel_path, parcel_data).await
            .with_context(|| format!("Error caching parcel {} at {}", parcel.label.name, parcel_path.display()))
    }

    pub async fn emplace_as_asset(&self, client: &bindle::client::Client, invoice_id: &bindle::Id, parcel: &bindle::Parcel) -> anyhow::Result<()> {
        let parcel_path = self.asset_parcel_path(invoice_id, parcel);
        if parcel_path.is_file() {
            return Ok(());
        }

        let parcel_data = client.get_parcel(invoice_id, &parcel.label.sha256).await
            .with_context(|| format!("Error fetching remote parcel {}", parcel.label.name))?;
        safely_write(&parcel_path, parcel_data).await
            .with_context(|| format!("Error caching parcel {} at {}", parcel.label.name, parcel_path.display()))?;
        Ok(())
    }

    async fn emplace_as_assets(&self, client: &bindle::client::Client, invoice_id: &bindle::Id, parcels: &Vec<bindle::Parcel>) -> anyhow::Result<()> {
        let placement_futures = parcels.iter().map(|parcel| self.emplace_as_asset(client, invoice_id, parcel));
        let all_placements = futures::future::join_all(placement_futures).await;
        let first_error = all_placements.into_iter().find(|p| p.is_err());
        first_error.unwrap_or(Ok(()))
    }

    // TODO: there is a potential risk here if two bindle servers have different content
    // for the same invoice id - if we cached data from the 'old' server we would use that
    // in place of the new one
    fn invoice_path(&self, invoice_id: &bindle::Id) -> PathBuf {
        let filename = invoice_cache_key(invoice_id);
        self.invoices_path().join(filename)
    }

    fn module_parcel_path(&self, parcel: &bindle::Parcel) -> PathBuf {
        self.cache_path.join(&parcel.label.sha256)
    }

    fn asset_parcel_path(&self, invoice_id: &bindle::Id, parcel: &bindle::Parcel) -> PathBuf {
        self.asset_path_for(invoice_id).join(&parcel.label.name)
    }

    fn invoices_path(&self) -> PathBuf {
        self.cache_path.join("_INVOICES")
    }

    fn asset_path(&self) -> PathBuf {
        self.cache_path.join("_ASSETS")
    }

    pub fn asset_path_for(&self, invoice_id: &bindle::Id) -> PathBuf {
        let key = invoice_cache_key(invoice_id);
        self.asset_path().join(key)
    }

    fn asset_dir_volume_mount(&self, invoice_id: &bindle::Id) -> HashMap<String, String> {
        let mut volumes = HashMap::new();
        volumes.insert("/".to_owned(), self.asset_path_for(invoice_id).display().to_string());  // TODO: maybe volumes should map PathBufs // or struct of host and guest
        volumes
    }
    
}

pub(crate) fn invoice_cache_key(id: &bindle::Id) -> String {
    let invoice_id_string = format!("{}/{}", id.name(), id.version_string());
    let mut hasher = Sha256::new();
    hasher.update(invoice_id_string);
    let result = hasher.finalize();
    format!("{:x}", result)
}

async fn safely_write(path: impl AsRef<Path>, content: Vec<u8>) -> std::io::Result<()> {
    let path = path.as_ref();
    let dir = path.parent().ok_or_else(||
        std::io::Error::new(std::io::ErrorKind::Other, format!("cache location {} has no parent directory", path.display()))
    )?;
    tokio::fs::create_dir_all(dir).await?;
    tokio::fs::write(path, content).await
}
