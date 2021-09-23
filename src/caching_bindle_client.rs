// use std::path::{Path, PathBuf};

// use sha2::{Digest, Sha256};

// pub struct CachingBindleClient {
//     bindle_client: bindle::client::Client,
//     cache_path: PathBuf,
// }

// impl CachingBindleClient {
//     pub fn new(bindle_server: &url::Url, cache_path: impl AsRef<Path>) -> Result<Self, bindle::client::ClientError> {
//         let bindle_client = bindle::client::Client::new(&bindle_server.to_string())?;
//         Ok(Self {
//             bindle_client,
//             cache_path: cache_path.as_ref().to_path_buf()
//         })
//     }

    // fn asset_path(&self) -> PathBuf {
    //     self.cache_path.join("_ASSETS")
    // }

    // pub fn asset_path_for(&self, invoice_id: &bindle::Id) -> PathBuf {
    //     let key = invoice_asset_cache_key(invoice_id);
    //     self.asset_path().join(key)
    // }

    // pub async fn get_invoice(&self, invoice_id: &bindle::Id) -> Result<bindle::Invoice, bindle::client::ClientError> {
    //     self.bindle_client.get_invoice(invoice_id).await
    // }

    // pub async fn get_module_parcel(&self, invoice_id: &bindle::Id, parcel: &bindle::Parcel) -> Result<Vec<u8>, bindle::client::ClientError> {
    //     let parcel_path = self.cache_path.join(&parcel.label.sha256);
    //     if parcel_path.is_file() {
    //         let parcel_data = tokio::fs::read(parcel_path).await?;
    //         return Ok(parcel_data);
    //     }

    //     tokio::fs::create_dir_all(&self.cache_path).await?;
    //     let parcel_data = self.bindle_client.get_parcel(invoice_id, &parcel.label.sha256).await?;
    //     tokio::fs::write(parcel_path, parcel_data.clone()).await?;
    //     Ok(parcel_data)
    // }

    // pub async fn emplace_asset_parcel(&self, invoice_id: &bindle::Id, parcel: &bindle::Parcel) -> anyhow::Result<()> {
    //     let base_dir = self.asset_path_for(invoice_id);
    //     let parcel_path = base_dir.join(&parcel.label.name);
    //     if parcel_path.is_file() {
    //         return Ok(());
    //     }

    //     let parcel_dir = parcel_path.parent().ok_or_else(|| anyhow::anyhow!("Can't emplace {} at {}: no parent directory", parcel.label.name, parcel_path.display()))?;
    //     tokio::fs::create_dir_all(parcel_dir).await?;
    //     let parcel_data = self.bindle_client.get_parcel(invoice_id, &parcel.label.sha256).await?;
    //     tokio::fs::write(parcel_path, parcel_data).await?;
    //     Ok(())
    // }

    // pub async fn emplace_asset_parcels(&self, invoice_id: &bindle::Id, parcels: &[&bindle::Parcel]) -> anyhow::Result<()> {
    //     let placement_futures = parcels.iter().map(|parcel| self.emplace_asset_parcel(invoice_id, parcel));
    //     let all_placements = futures::future::join_all(placement_futures).await;
    //     let first_error = all_placements.into_iter().find(|p| p.is_err());
    //     first_error.unwrap_or(Ok(()))
    // }
// }

// pub(crate) fn invoice_asset_cache_key(id: &bindle::Id) -> String {
//     let invoice_id_string = format!("{}/{}", id.name(), id.version_string());
//     let mut hasher = Sha256::new();
//     hasher.update(invoice_id_string);
//     let result = hasher.finalize();
//     format!("{:x}", result)
// }
