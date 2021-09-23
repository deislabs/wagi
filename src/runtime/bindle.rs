// use std::{path::{Path, PathBuf}};

// use bindle::{client::Client, Id, Invoice, Parcel};
// use sha2::{Digest, Sha256};
// use tracing::{instrument, trace, warn};
// use url::Url;

// pub(crate) fn bindle_cache_key(uri: &Url) -> String {
//     let mut hasher = Sha256::new();
//     hasher.update(uri.path());
//     let result = hasher.finalize();
//     format!("{:x}", result)
// }

// /// Given a server and a URI, attempt to load the bindle identified in the URI.
// ///
// /// TODO: this currently fetches the first application/wasm condition-less parcel from the bindle and tries
// /// to load it.
// #[instrument(level = "info", skip(engine))]
// pub(crate) async fn load_bindle(
//     server: &str,
//     uri: &Url,
//     engine: &wasmtime::Engine,
//     cache: &Path,
// ) -> anyhow::Result<wasmtime::Module> {
//     let bindle_name = uri.path();

//     tracing::debug!(
//         %bindle_name,
//         "Loading bindle",
//     );

//     let bindler = Client::new(server)?;
//     let invoice = bindler.get_invoice(bindle_name).await?;

//     // TODO: We need to load a keyring and then get it all the way here.
//     //invoice.verify(keyring)

//     // TODO: We should probably turn on the LRU.

//     tracing::trace!(
//         parcels = %invoice
//             .parcel
//             .clone()
//             .unwrap_or_default()
//             .iter()
//             .map(|p| p.label.name.clone())
//             .collect::<Vec<_>>()
//             .join(","),
//         "All bindle parcels",
//     );

//     // For now, we grab a list of parcels that have no conditions.
//     // This is definitely not the best strategy.
//     let parcels = invoice.parcel;
//     let to_fetch: Vec<Parcel> = parcels
//         .unwrap_or_default()
//         .iter()
//         .filter(|parcel| {
//             if parcel.label.media_type.as_str() == WASM_MEDIA_TYPE {
//                 let is_default = parcel.is_global_group();
//                 if !is_default {
//                     warn!("The parcel {} is not in the default group (it has a non-empty memberOf), and is ignored.", parcel.label.name);
//                 }
//                 return is_default
//             }
//             false
//         })
//         .cloned()
//         .collect();

//     tracing::trace!(
//         candidates = %to_fetch
//             .clone()
//             .iter()
//             .map(|p| p.label.name.clone())
//             .collect::<Vec<_>>()
//             .join(","),
//         "Module candidates",
//     );

//     if to_fetch.is_empty() {
//         tracing::error!("No parcels were module candidates");
//         anyhow::bail!("No suitable parcel found");
//     }

//     let first = to_fetch.get(0).unwrap();

//     tracing::trace!(parcel_name = %first.label.name, "Fetching module parcel");
//     let p = bindler
//         .get_parcel(bindle_name, first.label.sha256.as_str())
//         .await
//         .map_err(|e| {
//             tracing::error!(error = %e, "Error downloading parcel");
//             e
//         })?;

//     tracing::trace!("Writing module parcel to cache");
//     if let Err(e) = tokio::fs::write(cache.join(invoice.bindle.id.to_string()), &p).await {
//         tracing::warn!(error = %e, "Failed to cache bindle")
//     }
//     wasmtime::Module::new(engine, p)
// }

// pub async fn load_parcel(
//     server: &str,
//     uri: &Url,
//     engine: &wasmtime::Engine,
//     cache: &Path,
// ) -> anyhow::Result<wasmtime::Module> {
//     let bindler = Client::new(server)?;
//     let parcel_sha = uri.fragment();
//     if parcel_sha.is_none() {
//         anyhow::bail!("No parcel sha was found in URI: {}", uri)
//     }

//     let p = load_parcel_asset(&bindler, uri).await?;
//     if let Err(e) = tokio::fs::write(cache.join(parcel_sha.unwrap()), &p).await {
//         tracing::warn!("Failed to cache bindle: {}", e)
//     }
//     wasmtime::Module::new(engine, p)
// }

// /// Load a parcel, but make no assumptions about what is in the parcel.
// pub async fn load_parcel_asset(bindler: &Client, uri: &Url) -> anyhow::Result<Vec<u8>> {
//     let bindle_name = uri.path();
//     let parcel_sha = uri.fragment();
//     if parcel_sha.is_none() {
//         anyhow::bail!("No parcel sha was found in URI: {}", uri)
//     }
//     trace!("fetching parcel asset from bindle server");
//     let r = bindler.get_parcel(bindle_name, parcel_sha.unwrap()).await?;
//     trace!("received parcel");
//     Ok(r)
// }

// /// Copy a parcel from a bindle directory into cache and then return the path to the cached
// /// version.
// ///
// /// Wagi creates a local cache of all of the file assets for a particular bindle.  These assets are
// /// stored in a directory, and then during exection of a module, the directory is mounted to the
// /// wasm module as `/`.
// ///
// /// This is part of a workaround for Wasmtime. When Wasmtime can mount files directly, this method
// /// will be removed and the runtime will mount files directly from the parcel.
// pub async fn copy_parcel_asset(
//     local_path: PathBuf,
//     uri: &Url,
//     asset_cache: PathBuf,
//     guest_path: String,
// ) -> anyhow::Result<PathBuf> {
//     trace!("caching parcel as asset");
//     let hash = bindle_cache_key(&uri);
//     let dest = asset_cache.join(hash);

//     // Now we can create the cache directory.
//     // If it already exists, create_dir_all will not return an error.
//     tokio::fs::create_dir_all(&dest).await.map_err(|e| {
//         anyhow::anyhow!(
//             "Could not create asset cache directory at {}: {}",
//             dest.display(),
//             e
//         )
//     })?;

//     // Next, we dump the parcel into the cache directory, creating directories as needed.
//     let internal_path = dest.join(guest_path);
//     if !internal_path.starts_with(&dest) {
//         anyhow::bail!(
//             "Attempt to breakout of cache: Parcel tried to write to {}",
//             internal_path.display()
//         );
//     }

//     // We have already checked to make sure there is no breakout.
//     // So now we are just looking to make sure that the parent directory exists.
//     let parent = internal_path.parent().unwrap_or(&dest);
//     tokio::fs::create_dir_all(parent)
//         .await
//         .map_err(|e| anyhow::anyhow!("Failed to create asset subdirectories: {}", e))?;

//     tokio::fs::copy(local_path, internal_path).await?;
//     Ok(dest)
// }

// /// Load a parcel, cache it locally on disk, and then return the path to the cached version.
// ///
// /// Wagi creates a local cache of all of the file assets for a particular bindle.
// /// These assets are stored in a directory, and then during exection of a module,
// /// the directory is mounted to the wasm module as `/`.
// ///
// /// This is part of a workaround for Wasmtime. When Wasmtime can be safely used in
// /// async, this method will be removed and the runtime will directly load from the parcel.
// pub async fn cache_parcel_asset(
//     bindler: &Client,
//     uri: &Url,
//     asset_cache: PathBuf,
//     guest_path: String,
// ) -> anyhow::Result<PathBuf> {
//     trace!("caching parcel as asset");
//     let hash = bindle_cache_key(&uri);
//     let dest = asset_cache.join(hash);

//     // Now we can create the cache directory.
//     // If it already exists, create_dir_all will not return an error.
//     tokio::fs::create_dir_all(&dest).await.map_err(|e| {
//         anyhow::anyhow!(
//             "Could not create asset cache directory at {}: {}",
//             dest.display(),
//             e
//         )
//     })?;

//     // Next, we dump the parcel into the cache directory, creating directories as needed.
//     let internal_path = dest.join(guest_path);
//     if !internal_path.starts_with(&dest) {
//         anyhow::bail!(
//             "Attempt to breakout of cache: Parcel tried to write to {}",
//             internal_path.display()
//         );
//     }

//     // We have already checked to make sure there is no breakout.
//     // So now we are just looking to make sure that the parent directory exists.
//     let parent = internal_path.parent().unwrap_or(&dest);
//     tokio::fs::create_dir_all(parent)
//         .await
//         .map_err(|e| anyhow::anyhow!("Failed to create asset subdirectories: {}", e))?;

//     // Next, we fetch the actual data from the Bindle server and then write it to
//     // the newly created path on disk.
//     let data = load_parcel_asset(bindler, uri).await?;
//     tokio::fs::write(&internal_path, data)
//         .await
//         .map_err(|e| anyhow::anyhow!("Could not create parcel data file: {}", e))?;

//     Ok(dest)
// }

// TODO: this potentially relates to `bindle:` URIs in module refs?
// /// Convenience function for generating an internal Parcel URL.
// ///
// /// Internally, a parcel URL is represented as `parcel:$NAME/$VERSION#$PARCEL_SHA`
// /// This is not a general convention, but is used to pass parcel information into
// /// and out of a module configuration.
// fn parcel_url(bindle_id: &Id, parcel_sha: String) -> String {
//     format!(
//         "parcel:{}/{}#{}",
//         bindle_id.name(),
//         bindle_id.version_string(),
//         parcel_sha
//     )
// }

// TODO: need bring dependency groups code across
/*
/// Given a bindle's invoice and parcel directory, build a module configuration.
pub async fn standalone_invoice_to_modules(
    server_uri: &str,
    invoice: &Invoice,
    parcel_dir: PathBuf,
    asset_cache: PathBuf,
) -> anyhow::Result<ModuleConfig> {
    let mut modules = IndexSet::new();
    let bindle_id = invoice.bindle.id.clone();

    // For each top-level entry, if it is a Wasm module, we create a Module.
    let top = top_modules(invoice);
    debug!(
        default_modules = top.len(),
        "Loaded modules from the default group (parcels that do not have conditions.memberOf set)"
    );

    for parcel in top {
        // Create a basic module definition from the features section on this parcel.
        let mpath = parcel_dir
            .join(format!("{}.dat", parcel.label.sha256))
            .to_string_lossy()
            .to_string();
        let parcel_bytes = tokio::fs::read(mpath).await?;
        let mut def = wagi_features(&invoice.bindle.id, &parcel, parcel_bytes);

        def.module = parcel_dir
            .join(format!("{}.dat", parcel.label.sha256))
            .to_string_lossy()
            .to_string();

        // If the parcel has a group, get the group.
        // Then we have to figure out how to map the group onto a Wagi configuration.
        if let Some(c) = parcel.conditions.clone() {
            let groups = c.requires.unwrap_or_default();
            for name in groups.into_iter() {
                let members = group_members(invoice, name.as_str());

                // If it is a file, then we will mount it as a volume
                for member in members {
                    if is_file(&member) {
                        // Store the parcel at a local path
                        let purl = parcel_url(&bindle_id, member.label.sha256.clone());
                        trace!(parcel = %purl, "converting a parcel to an asset");
                        let puri = purl.parse().unwrap();

                        let local_path = parcel_dir.join(format!("{}.dat", member.label.sha256));

                        let cached_path = copy_parcel_asset(
                            local_path,
                            &puri,
                            asset_cache.clone(),
                            member.label.name.clone(),
                        )
                        .await?;

                        // Right now, we have to cache all of the files locally in one
                        // directory and then mount that entire directory synchronously
                        // (as a detail of how wasmtime currently works).
                        // So for now, all we need to do is point Wagi to the directory
                        // and have it mount that directory as root.
                        //
                        // The directory that cache_parcel_asset returns is the directory
                        // that we expect all files to be written to. So we map
                        // that to `/`
                        if def.volumes.is_none() {
                            let mut volumes = HashMap::new();
                            volumes
                                .insert("/".to_owned(), cached_path.to_str().unwrap().to_owned());
                            def.volumes = Some(volumes);
                        }
                        trace!("Done with conversion");
                    }
                }

                // Currently, there are no other defined behaviors for parcels.
            }
        }

        // For each group required by the module entry, we try to map its parts to one
        // or more of the Bindle module details

        modules.insert(def);
    }

    // Finally, we return the module configuration
    let mc = ModuleConfig {
        route_cache: None, // This is built by ModuleConfig.build_registry(), which is called later.
        modules,
    };

    Ok(mc)
}
/// Given a bindle's invoice, build a module configuration.
pub async fn invoice_to_modules(
    server_uri: &str,
    invoice: &Invoice,
    bindle_server: &str,
    asset_cache: PathBuf,
) -> anyhow::Result<ModuleConfig> {
    let mut modules = IndexSet::new();
    let bindle_id = invoice.bindle.id.clone();

    // For each top-level entry, if it is a Wasm module, we create a Module.
    let top = top_modules(invoice);
    debug!(
        default_modules = top.len(),
        "Loaded modules from the default group (parcels that do not have conditions.memberOf set)"
    );

    for parcel in top {
        // Create a basic module definition from the features section on this parcel.
        let bindler_temp  = Client::new(bindle_server)?;
        let parcel_bytes = bindler_temp.get_parcel(&invoice.bindle.id, &parcel.label.sha256).await?;
        let mut def = wagi_features(&invoice.bindle.id, &parcel, parcel_bytes);

        // FIXME: This should get refactored out. Right now, every module needs its own
        // reference to a bindle server. This is because in the older modules.toml
        // format, it is legal to specify a different bindle server per modules. And
        // THIS is because the original modules.toml was designed to support multi-tenancy.
        // As we slim down the scope of Wagi, we should probably refactor this assumption
        // out of the codebase.
        def.bindle_server = Some(bindle_server.to_owned());
        let bindler = Client::new(bindle_server)?;

        // If the parcel has a group, get the group.
        // Then we have to figure out how to map the group onto a Wagi configuration.
        if let Some(c) = parcel.conditions.clone() {
            let groups = c.requires.unwrap_or_default();
            for n in groups.iter() {
                let name = n.clone();
                let members = group_members(invoice, name.as_str());

                // If it is a file, then we will mount it as a volume
                for member in members {
                    if is_file(&member) {
                        // Store the parcel at a local path
                        let purl = parcel_url(&bindle_id, member.label.sha256.clone());
                        trace!(parcel = %purl, "converting a parcel to an asset");
                        let puri = purl.parse().unwrap();

                        // The returned cache path is the asset cache path PLUS the SHA256 of
                        // the parcel that contains this asset. Essentially, we are mapping
                        // the `/` path to `_ASSETS/$PARCEL_SHA` and then storing all the
                        // files for that parcel in the same directory.
                        let cache_path = cache_parcel_asset(
                            &bindler,
                            &puri,
                            asset_cache.clone(),
                            member.label.name.clone(),
                        )
                        .await?;

                        // Right now, we have to cache all of the files locally in one
                        // directory and then mount that entire directory synchronously
                        // (as a detail of how wasmtime currently works).
                        // So for now, all we need to do is point Wagi to the directory
                        // and have it mount that directory as root.
                        //
                        // The directory that cache_parcel_asset returns is the directory
                        // that we expect all files to be written to. So we map
                        // that to `/`
                        if def.volumes.is_none() {
                            let mut volumes = HashMap::new();
                            volumes.insert("/".to_owned(), cache_path.to_str().unwrap().to_owned());
                            def.volumes = Some(volumes);
                        }
                        trace!("Done with conversion");
                    }
                }

                // Currently, there are no other defined behaviors for parcels.
            }
        }

        // For each group required by the module entry, we try to map its parts to one
        // or more of the Bindle module details

        modules.insert(def);
    }

    // Finally, we return the module configuration
    let mc = ModuleConfig {
        route_cache: None, // This is built by ModuleConfig.build_registry(), which is called later.
        modules,
    };

    Ok(mc)
}
*/

// TODO: replace these functions.  (Currently used by tests.)
// fn group_members(invoice: &Invoice, name: &str) -> Vec<Parcel> {
//     invoice
//         .parcel
//         .clone()
//         .unwrap_or_default()
//         .iter()
//         .filter(|p| p.member_of(name))
//         .cloned()
//         .collect()
// }

// fn is_file(parcel: &Parcel) -> bool {
//     let wagi_key = "wagi".to_owned();
//     let file_key = "file".to_owned();
//     parcel
//         .label
//         .feature
//         .as_ref()
//         .map(|f| {
//             f.get(&wagi_key).map(|g| match g.get(&file_key) {
//                 Some(v) => "true" == v,
//                 None => false,
//             })
//         })
//         .flatten()
//         .unwrap_or(false)
// }
