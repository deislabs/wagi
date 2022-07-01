use std::{path::Path, sync::Arc};

use anyhow::Context;
// TODO: move OCI-specific stuff out to a helper file
use docker_credential::DockerCredential;
use oci_distribution::client::{Client, ClientConfig};
use oci_distribution::secrets::RegistryAuth;
use oci_distribution::Reference;
use sha2::{Digest, Sha256};
use url::Url;

use crate::wagi_config::WagiConfiguration;

use super::loader::ModuleMapConfigurationEntry;

pub async fn load_from_module_map_entry(
    module_map_entry: &ModuleMapConfigurationEntry,
    configuration: &WagiConfiguration,
) -> anyhow::Result<Vec<u8>> {
    let module_ref = module_map_entry.module.clone();
    match url::Url::parse(&module_ref) {
        Err(e) => {
            tracing::debug!(
                error = %e,
                "Error parsing module URI. Assuming this is a local file"
            );
            let bytes = tokio::fs::read(&module_ref).await.with_context(|| {
                format!(
                    "Error reading file '{}' referenced by module config",
                    module_ref
                )
            })?;
            Ok(bytes)
        }
        Ok(uri) => match uri.scheme() {
            "file" => match uri.to_file_path() {
                Ok(p) => Ok(tokio::fs::read(&p).await.with_context(|| {
                    format!(
                        "Error reading file '{}' referenced by module file: URI",
                        p.display()
                    )
                })?),
                Err(e) => Err(anyhow::anyhow!(
                    "Cannot get path to file {}: {:#?}",
                    module_ref,
                    e
                )),
            },
            "bindle" => {
                // TODO: should we allow --bindle-server so modules.toml can resolve?  This is deprecated so not keen
                let bindle_server = module_map_entry.bindle_server.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("No Bindle server specified for module {}", module_ref)
                })?;
                load_bindle(bindle_server, &uri, &configuration.asset_cache_dir).await
            }
            // "parcel" => self.load_parcel(&uri, store.engine(), cache).await,  // TODO: this is not mentioned in the spec...?
            "oci" => load_from_oci(&uri, &configuration.asset_cache_dir).await,
            s => Err(anyhow::anyhow!(
                "Unknown scheme {} in module reference {}",
                s,
                module_ref
            )),
        },
    }
}

const WASM_LAYER_CONTENT_TYPE: &str = "application/vnd.wasm.content.layer.v1+wasm";

#[tracing::instrument(level = "info", skip(cache))]
async fn load_from_oci(uri: &url::Url, cache: impl AsRef<Path>) -> anyhow::Result<Vec<u8>> {
    let cache_file_name = hash_name(uri);
    let cache_file_path = cache.as_ref().join(cache_file_name);

    if cache_file_path.is_file() {
        if let Ok(bytes) = tokio::fs::read(&cache_file_path).await {
            return Ok(bytes);
        }
    }

    let config = ClientConfig {
        protocol: oci_distribution::client::ClientProtocol::HttpsExcept(vec![
            "localhost:5000".to_owned(),
            "127.0.0.1:5000".to_owned(),
        ]),
    };
    let mut oc = Client::new(config);

    let mut auth = RegistryAuth::Anonymous;

    if let Ok(DockerCredential::UsernamePassword(user_name, password)) =
        docker_credential::get_credential(uri.as_str())
    {
        auth = RegistryAuth::Basic(user_name, password);
    };

    let img = url_to_oci(uri)
        .map_err(|e| {
            tracing::error!(
                error = %e,
                "Could not convert uri to OCI reference"
            );
            e
        })
        .with_context(|| format!("Could not convert URI '{}' to OCI reference", uri))?;
    let data = oc
        .pull(&img, &auth, vec![WASM_LAYER_CONTENT_TYPE])
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Pull failed");
            e
        })
        .with_context(|| format!("Failed to pull OCI artifact {}", img))?;
    if data.layers.is_empty() {
        tracing::error!(image = %img, "Image has no layers");
        anyhow::bail!("image {} has no layers", img);
    }
    let first_layer = data.layers.get(0).unwrap();
    let bytes = first_layer.data.clone();

    // If a cache write fails, log it but continue on.
    tracing::trace!("writing layer to module cache");
    if let Err(e) = safely_write(&cache_file_path, &bytes).await {
        tracing::warn!(error = %e, "failed to write module to cache");
    }

    Ok(bytes)
}

fn url_to_oci(uri: &Url) -> anyhow::Result<Reference> {
    let name = uri.path().trim_start_matches('/');
    let port = uri.port().map(|p| format!(":{}", p)).unwrap_or_default();
    let r: Reference = match uri.host() {
        Some(host) => format!("{}{}/{}", host, port, name).parse(),
        None => name.parse(),
    }?;
    Ok(r) // Because who doesn't love OKRs.
}

// NOTE: load_bindle is copied with minor modifications from pre-refactor source.
// It has not been tidied because we expect to deprecate this.

/// Given a server and a URI, attempt to load the bindle identified in the URI.
///
/// TODO: this currently fetches the first application/wasm condition-less parcel from the bindle and tries
/// to load it.
#[tracing::instrument(level = "info", skip(cache))]
async fn load_bindle(
    server: &str,
    uri: &url::Url,
    cache: impl AsRef<Path>,
) -> anyhow::Result<Vec<u8>> {
    let cache_file_name = hash_name(uri);
    let cache_file_path = cache.as_ref().join(cache_file_name);

    if cache_file_path.is_file() {
        if let Ok(bytes) = tokio::fs::read(&cache_file_path).await {
            return Ok(bytes);
        }
    }

    let bindle_name = uri.path();

    tracing::debug!(
        %bindle_name,
        "Loading bindle",
    );
    let token = bindle::client::tokens::NoToken::default();
    let bindler = bindle::client::Client::new(server, token)?;
    let invoice = bindler.get_invoice(bindle_name).await?;

    // TODO: We need to load a keyring and then get it all the way here.
    //invoice.verify(keyring)

    // TODO: We should probably turn on the LRU.

    tracing::trace!(
        parcels = %invoice
            .parcel
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|p| p.label.name.clone())
            .collect::<Vec<_>>()
            .join(","),
        "All bindle parcels",
    );

    // For now, we grab a list of parcels that have no conditions.
    // This is definitely not the best strategy.
    let parcels = invoice.parcel;
    let to_fetch: Vec<bindle::Parcel> = parcels
        .unwrap_or_default()
        .iter()
        .filter(|parcel| {
            if parcel.label.media_type.as_str() == crate::bindle_util::WASM_MEDIA_TYPE {
                let is_default = parcel.is_global_group();
                if !is_default {
                    tracing::warn!("The parcel {} is not in the default group (it has a non-empty memberOf), and is ignored.", parcel.label.name);
                }
                return is_default
            }
            false
        })
        .cloned()
        .collect();

    tracing::trace!(
        candidates = %to_fetch
            .clone()
            .iter()
            .map(|p| p.label.name.clone())
            .collect::<Vec<_>>()
            .join(","),
        "Module candidates",
    );

    if to_fetch.is_empty() {
        tracing::error!("No parcels were module candidates");
        anyhow::bail!("No suitable parcel found in module ref '{}'", uri);
    }

    let first = to_fetch.get(0).unwrap();

    tracing::trace!(parcel_name = %first.label.name, "Fetching module parcel");
    let bytes = bindler
        .get_parcel(bindle_name, first.label.sha256.as_str())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Error downloading parcel");
            e
        })?;

    tracing::trace!("Writing module parcel to cache");
    if let Err(e) = safely_write(&cache_file_path, &bytes).await {
        tracing::warn!(error = %e, "Failed to cache bindle")
    }

    Ok(bytes)
}

fn hash_name(url: &Url) -> String {
    let mut hasher = Sha256::new();
    hasher.update(&url.as_str());
    let result = hasher.finalize();
    format!("{:x}", result)
}

// TODO: this is copied from `emplacer`*.  As emplacer is effectively a cache manager,
// we should look at combining this module with that (in whatever suitable way).
// Leaving this for now, though, until we figure out what we are deprecating (and
// so this refactor doesn't go on forever).
//
// *Except I changed it to take an &Vec instead of a Vec but I am sure our mighty
// brains can reconcile that if and when the moment comes.
async fn safely_write(path: impl AsRef<Path>, content: &[u8]) -> std::io::Result<()> {
    let path = path.as_ref();
    let dir = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("cache location {} has no parent directory", path.display()),
        )
    })?;
    tokio::fs::create_dir_all(dir).await?;
    tokio::fs::write(path, content).await
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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_url_to_oci() {
        let uri = url::Url::parse("oci:foo:bar").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("foo:bar", oci.whole().as_str());

        let uri = url::Url::parse("oci://example.com/foo:dev").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("example.com/foo:dev", oci.whole().as_str());

        let uri = url::Url::parse("oci:example/foo:1.2.3").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("example/foo:1.2.3", oci.whole().as_str());

        let uri = url::Url::parse("oci://example.com/foo:dev").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("example.com/foo:dev", oci.whole().as_str());

        let uri = url::Url::parse("oci://example.com:9000/foo:dev").expect("parse URL");
        let oci = url_to_oci(&uri).expect("parsing the URL should succeed");
        assert_eq!("example.com:9000/foo:dev", oci.whole().as_str());
    }
}
