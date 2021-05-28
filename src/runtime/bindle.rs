use std::path::PathBuf;

use bindle::{client::Client, Parcel};
use sha2::{Digest, Sha256};
use url::Url;
use wasmtime::{Engine, Module};

const WASM_MEDIA_TYPE: &str = "application/wasm";

pub(crate) fn bindle_cache_key(uri: &Url) -> String {
    let mut hasher = Sha256::new();
    hasher.update(uri.path());
    let result = hasher.finalize();
    format!("{:x}", result)
}

/// Given a server and a URI, attempt to load the bindle identified in the URI.
///
/// TODO: this currently fetches the first application/wasm condition-less parcel from the bindle and tries
/// to load it.
pub(crate) async fn load_bindle(
    server: &str,
    uri: &Url,
    engine: &Engine,
    cache: PathBuf,
) -> anyhow::Result<wasmtime::Module> {
    let bindle_name = uri.path();

    log::debug!(
        "load_bindle: Loading bindle {} from {}",
        bindle_name,
        server
    );

    let bindler = Client::new(server)?;
    let invoice = bindler.get_invoice(bindle_name).await?;

    // TODO: We need to load a keyring and then get it all the way here.
    //invoice.verify(keyring)

    // TODO: We should probably turn on the LRU.

    log::trace!(
        "load_bindle: All bindle parcels: [{}]",
        invoice
            .parcel
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|p| p.label.name.clone())
            .collect::<Vec<_>>()
            .join(",")
    );

    // For now, we grab a list of parcels that have no conditions.
    // This is definitely not the best strategy.
    let parcels = invoice.parcel;
    let to_fetch: Vec<Parcel> = parcels
        .unwrap_or_default()
        .iter()
        .filter(|parcel| {
            parcel.label.media_type.as_str() == WASM_MEDIA_TYPE && parcel.conditions.is_none()
        })
        .cloned()
        .collect();

    log::trace!(
        "load_bindle: Module candidates: [{}]",
        to_fetch
            .clone()
            .iter()
            .map(|p| p.label.name.clone())
            .collect::<Vec<_>>()
            .join(",")
    );

    if to_fetch.is_empty() {
        log::error!("load_bindle: No parcels were module candidates");
        anyhow::bail!("No suitable parcel found");
    }

    let first = to_fetch.get(0).unwrap();

    log::trace!("load_bindle: Fetching module parcel: {}", &first.label.name);
    let p = bindler
        .get_parcel(bindle_name, first.label.sha256.as_str())
        .await
        .map_err(|e| {
            log::error!("load_bindle: Error downloading parcel: {}", e);
            e
        })?;

    log::trace!("load_bindle: Writing module parcel to cache");
    tokio::fs::write(cache.join(invoice.bindle.id.to_string()), &p)
        .await
        .err()
        .map(|e| log::warn!("load_bindle: Failed to cache parcel: {}", e));
    Module::new(engine, p)
}
