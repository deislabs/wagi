use bindle::{client::Client, Parcel};
use url::Url;
use wasmtime::{Engine, Module};

const WASM_MEDIA_TYPE: &str = "application/wasm";

/// Given a server and a URI, attempt to load the bindle identified in the URI.
///
/// TODO: this currently fetches the first application/wasm condition-less parcel from the bindle and tries
/// to load it.
pub(crate) async fn load_bindle(server: &str, uri: &Url) -> anyhow::Result<wasmtime::Module> {
    let bindle_name = uri.path();
    let bindler = Client::new(server)?;
    let invoice = bindler.get_invoice(bindle_name).await?;

    // TODO: We need to load a keyring and then get it all the way here.
    //invoice.verify(keyring)

    // TODO: We should probably turn on the LRU.

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

    if to_fetch.is_empty() {
        anyhow::bail!("No suitable parcel found");
    }

    let first = to_fetch.get(0).unwrap();

    let p = bindler
        .get_parcel(invoice.bindle.id, first.label.sha256.as_str())
        .await?;
    Module::new(&Engine::default(), p)
}
