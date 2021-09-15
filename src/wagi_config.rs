use core::convert::TryFrom;
use std::{collections::HashMap, net::SocketAddr, path::{Path, PathBuf}};

use bindle::{Invoice, standalone::StandaloneRead};
use serde::Deserialize;

use crate::bindle_util;

#[derive(Clone, Debug)]
pub struct WagiConfiguration {
    pub handlers: HandlerConfigurationSource,
    pub env_vars: HashMap<String, String>,
    pub http_configuration: HttpConfiguration,
    pub wasm_cache_config_file: PathBuf,
    pub remote_module_cache_dir: PathBuf,
    pub log_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub enum HandlerConfigurationSource {
    ModuleConfigFile(PathBuf),
    StandaloneBindle(PathBuf, bindle::Id),
    RemoteBindle(url::Url, bindle::Id),
}

#[derive(Clone, Debug)]
pub struct HttpConfiguration {
    pub listen_on: SocketAddr,
    pub default_hostname: String,
    pub tls: Option<TlsConfiguration>,
}

#[derive(Clone, Debug)]
pub struct TlsConfiguration {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModuleMapConfiguration {
    #[serde(rename = "module")]
    pub entries: Vec<ModuleMapConfigurationEntry>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModuleMapConfigurationEntry {
    // The route to wire up
    pub route: String,
    // The Wasm to wire it up to
    pub module: String,  // file path, file://foo URL, bindle:foo/bar/1.2.3 or oci:foo/bar:1.2.3 (bindle: is deprecated which is good because it's not clear which parcel you'd use)
    pub entrypoint: Option<String>,
    pub bindle_server: Option<String>,
    // The environment in which to run it
    pub volumes: Option<HashMap<String, String>>,
    pub allowed_hosts: Option<Vec<String>>,
    pub http_max_concurrency: Option<u32>,
}

pub enum HandlerConfiguration {
    ModuleMapFile(ModuleMapConfiguration),
    Bindle(Invoice),
}

impl WagiConfiguration {
    pub async fn read_handler_configuration(&self) -> anyhow::Result<HandlerConfiguration> {
        match &self.handlers {
            HandlerConfigurationSource::ModuleConfigFile(path) =>
                read_module_map_configuration(path).await.map(HandlerConfiguration::ModuleMapFile),
            HandlerConfigurationSource::StandaloneBindle(path, bindle_id) =>
                read_standalone_bindle_invoice(path, bindle_id).await.map(HandlerConfiguration::Bindle),
            HandlerConfigurationSource::RemoteBindle(server_url, bindle_id) =>
                read_remote_bindle_invoice(server_url, bindle_id).await.map(HandlerConfiguration::Bindle),
        }
    }
}

async fn read_module_map_configuration(path: &PathBuf) -> anyhow::Result<ModuleMapConfiguration> {
    tracing::info!(?path, "Loading modules config file");
    if !tokio::fs::metadata(&path)
        .await
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        return Err(anyhow::anyhow!(
            "no modules configuration file found at {}",
            path.display()
        ));
    }

    let data = std::fs::read(path)?;
    let modules: ModuleMapConfiguration = toml::from_slice(&data)?;
    Ok(modules)
}

async fn read_standalone_bindle_invoice(path: impl AsRef<Path>, bindle_id: &bindle::Id) -> anyhow::Result<bindle::Invoice> {
    tracing::info!(%bindle_id, "Loading standalone bindle");
    let reader = StandaloneRead::new(path, bindle_id).await?;

    let data = tokio::fs::read(&reader.invoice_file).await?;
    let invoice: Invoice = toml::from_slice(&data)?;
    Ok(invoice)
}

async fn read_remote_bindle_invoice(server_url: &url::Url, bindle_id: &bindle::Id) -> anyhow::Result<bindle::Invoice> {
    tracing::info!(%bindle_id, "Loading remote bindle");
    let bindler = ::bindle::client::Client::new(&server_url.to_string())?;

    let invoice = bindler.get_invoice(bindle_id).await?;
    Ok(invoice)
}

pub struct RequiredBlob {
    pub source: BlobSource,
}

pub enum BlobSource {
    File(PathBuf),
    FileLegacy(PathBuf),
    Oci(String),
    BindleParcel(BindleParcel),
    BindleLegacy(url::Url, bindle::Id),
}

pub struct BindleParcel {
    pub name: String,
    pub sha256: String,
}

pub async fn required_blobs(handlers: &HandlerConfiguration) -> anyhow::Result<Vec<RequiredBlob>> {
    match handlers {
        HandlerConfiguration::ModuleMapFile(module_map_config) =>
            required_blobs_for_module_map(module_map_config),
        HandlerConfiguration::Bindle(invoice) =>
            required_blobs_for_bindle(invoice),
    }
}

fn required_blobs_for_module_map(module_map_config: &ModuleMapConfiguration) -> anyhow::Result<Vec<RequiredBlob>> {
    module_map_config.entries
        .iter()
        .map(|e| parse_module_ref(e))
        .collect()
}

fn parse_module_ref(module: &ModuleMapConfigurationEntry) -> anyhow::Result<RequiredBlob> {
    let module_ref = &module.module;
    match url::Url::parse(module_ref) {
        Err(e) => {
            tracing::debug!(
                error = %e,
                "Error parsing module URI. Assuming this is a local file"
            );
            Ok(RequiredBlob {
                source: BlobSource::FileLegacy(PathBuf::from(module_ref)),
            })
        },
        Ok(uri) => match uri.scheme() {
            "file" => match uri.to_file_path() {
                Ok(p) => Ok(RequiredBlob { source: BlobSource::File(p) }),
                Err(e) => Err(anyhow::anyhow!("Cannot get path to file {}: {:#?}", module_ref, e)),
            }
            "bindle" => {
                // TODO: should we allow --bindle-server so modules.toml can resolve?  This is deprecated so not keen
                let bindle_server = module.bindle_server.as_ref().ok_or_else(|| anyhow::anyhow!("No Bindle server specified for module {}", module_ref))?;
                let bindle_server_url = url::Url::parse(bindle_server)?;
                let bindle_id = bindle::Id::try_from(uri.path())?;
                Ok(RequiredBlob { source: BlobSource::BindleLegacy(bindle_server_url, bindle_id) })
            },
            // "parcel" => self.load_parcel(&uri, store.engine(), cache).await,  // TODO: this is not mentioned in the spec...?
            "oci" => Ok(RequiredBlob { source: BlobSource::Oci(uri.path().to_owned()) }),
            s => Err(anyhow::anyhow!("Unknown scheme {} in module reference {}", s, module_ref)),
        }
    }
}

fn required_blobs_for_bindle(invoice: &bindle::Invoice) -> anyhow::Result<Vec<RequiredBlob>> {
    // For each top-level entry, if it is a Wasm module, we create a Module.
    let top = crate::bindle_util::top_modules(invoice);
    tracing::debug!(
        default_modules = top.len(),
        "Loaded modules from the default group (parcels that do not have conditions.memberOf set)"
    );

    let routable_top_level_parcels: Vec<_> = top.iter().filter(|p| crate::bindle_util::is_wagi_handler(p)).collect();

    let dependencies = bindle_util::build_full_memberships(invoice);

    let required_parcels =
        routable_top_level_parcels
            .iter()
            .flat_map(|parcel| bindle_util::parcels_required_for(parcel, &dependencies));

    let required_blobs = required_parcels.map(|p| BindleParcel { name: p.label.name.to_owned(), sha256: p.label.sha256.to_owned() })
        .map(|bp| RequiredBlob { source: BlobSource::BindleParcel(bp) })
        .collect();

    Ok(required_blobs)
            // // If the parcel has a group, get the group.
            // // Then we have to figure out how to map the group onto a Wagi configuration.
            // if let Some(c) = parcel.conditions.clone() {
            //     let groups = c.requires.unwrap_or_default();
            //     for n in groups.iter() {
            //         let name = n.clone();
            //         let members = group_members(invoice, name.as_str());
    
            //         // If it is a file, then we will mount it as a volume
            //         for member in members {
            //             if is_file(&member) {
            //                 // Store the parcel at a local path
            //                 let purl = parcel_url(&bindle_id, member.label.sha256.clone());
            //                 trace!(parcel = %purl, "converting a parcel to an asset");
            //                 let puri = purl.parse().unwrap();
    
            //                 // The returned cache path is the asset cache path PLUS the SHA256 of
            //                 // the parcel that contains this asset. Essentially, we are mapping
            //                 // the `/` path to `_ASSETS/$PARCEL_SHA` and then storing all the
            //                 // files for that parcel in the same directory.
            //                 let cache_path = cache_parcel_asset(
            //                     &bindler,
            //                     &puri,
            //                     asset_cache.clone(),
            //                     member.label.name.clone(),
            //                 )
            //                 .await?;
    
            //                 // Right now, we have to cache all of the files locally in one
            //                 // directory and then mount that entire directory synchronously
            //                 // (as a detail of how wasmtime currently works).
            //                 // So for now, all we need to do is point Wagi to the directory
            //                 // and have it mount that directory as root.
            //                 //
            //                 // The directory that cache_parcel_asset returns is the directory
            //                 // that we expect all files to be written to. So we map
            //                 // that to `/`
            //                 if def.volumes.is_none() {
            //                     let mut volumes = HashMap::new();
            //                     volumes.insert("/".to_owned(), cache_path.to_str().unwrap().to_owned());
            //                     def.volumes = Some(volumes);
            //                 }
            //                 trace!("Done with conversion");
            //             }
            //         }
    
            //         // Currently, there are no other defined behaviors for parcels.
            //     }
            // }
    
            // // For each group required by the module entry, we try to map its parts to one
            // // or more of the Bindle module details
    
            // modules.insert(def);
        // }
    
}
