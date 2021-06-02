use std::{collections::HashMap, path::PathBuf};

use bindle::{client::Client, Id, Invoice, Parcel};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{runtime::Module, ModuleConfig};

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
    engine: &wasmtime::Engine,
    cache: PathBuf,
) -> anyhow::Result<wasmtime::Module> {
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
        .get_parcel(bindle_name, first.label.sha256.as_str())
        .await?;
    tokio::fs::write(cache.join(invoice.bindle.id.to_string()), &p)
        .await
        .err()
        .map(|e| log::warn!("Failed to cache bindle: {}", e));
    wasmtime::Module::new(engine, p)
}

pub async fn load_parcel(
    server: &str,
    uri: &Url,
    engine: &wasmtime::Engine,
    cache: PathBuf,
) -> anyhow::Result<wasmtime::Module> {
    let bindler = Client::new(server)?;
    let parcel_sha = uri.fragment();
    if parcel_sha.is_none() {
        anyhow::bail!("No parcel sha was found in URI: {}", uri)
    }

    let p = load_parcel_asset(&bindler, uri).await?;
    tokio::fs::write(cache.join(parcel_sha.unwrap()), &p)
        .await
        .err()
        .map(|e| log::warn!("Failed to cache bindle: {}", e));
    wasmtime::Module::new(engine, p)
}

/// Load a parcel, but make no assumptions about what is in the parcel.
pub async fn load_parcel_asset(bindler: &Client, uri: &Url) -> anyhow::Result<Vec<u8>> {
    let bindle_name = uri.path();
    let parcel_sha = uri.fragment();
    if parcel_sha.is_none() {
        anyhow::bail!("No parcel sha was found in URI: {}", uri)
    }
    let r = bindler.get_parcel(bindle_name, parcel_sha.unwrap()).await?;
    Ok(r)
}

/// Fetch a bindle and convert it to a module configuration.
pub async fn bindle_to_modules(name: &str, server_url: &str) -> anyhow::Result<ModuleConfig> {
    let bindler = Client::new(server_url)?;
    let invoice = bindler.get_invoice(name).await?;

    Ok(invoice_to_modules(&invoice, server_url))
}

fn parcel_url(bindle_id: &Id, parcel_sha: String) -> String {
    format!(
        "parcel:{}/{}#{}",
        bindle_id.name(),
        bindle_id.version_string(),
        parcel_sha
    )
}

/// Given a bindle's invoice, build a module configuration.
pub fn invoice_to_modules(invoice: &Invoice, bindle_server: &str) -> ModuleConfig {
    let mut modules = vec![];
    let bindle_id = invoice.bindle.id.clone();

    // For each top-level entry, if it is a Wasm module, we create a Module.
    let top = top_modules(invoice);

    for parcel in top {
        // Create a basic module definition from the features section on this parcel.
        let mut def = wagi_features(&invoice.bindle.id, &parcel);

        // FIXME: This should get refactored out. Right now, every module needs its own
        // reference to a bindle server. This is because in the older modules.toml
        // format, it is legal to specify a different bindle server per modules. And
        // THIS is because the original modules.toml was designed to support multi-tenancy.
        // As we slim down the scope of Wagi, we should probably refactor this assumption
        // out of the codebase.
        def.bindle_server = Some(bindle_server.to_owned());

        // If the parcel has a group, get the group.
        // Then we have to figure out how to map the group onto a Wagi configuration.
        if let Some(c) = parcel.conditions.clone() {
            let mut volumes = HashMap::new();
            let groups = c.requires.unwrap_or_default();
            groups.iter().for_each(|n| {
                let name = n.clone();
                let members = group_members(invoice, name.as_str());

                // If it is a file, then we will mount it as a volume
                for member in members {
                    if is_file(&member) {
                        // Add this parcel to the volumes
                        volumes.insert(
                            parcel.label.name.clone(),
                            parcel_url(&bindle_id, parcel.label.sha256.clone()),
                        );
                    }
                }

                // If there are any volumes, then we add them to the definition
                if !volumes.is_empty() {
                    def.volumes = Some(volumes.clone());
                }
                // Currently, there are no other defined behaviors for parcels.
            });
        }

        // For each group required by the module entry, we try to map its parts to one
        // or more of the Bindle module details

        modules.push(def)
    }

    // Finally, we return the module configuration
    let mc = ModuleConfig {
        default_host: None, // TODO: Do we care about this?
        route_cache: None,  // TODO: Pass this in
        modules,
    };

    mc
}

// America's next...
fn top_modules(inv: &Invoice) -> Vec<Parcel> {
    inv.parcel
        .clone()
        .unwrap_or_default()
        .iter()
        .filter(|parcel| {
            // We want parcels that...
            // - have the Wasm media type
            // - Have no group memberships
            parcel.label.media_type.as_str() == WASM_MEDIA_TYPE && parcel.is_global_group()
        })
        .cloned()
        .collect()
}

fn wagi_features(inv_id: &Id, parcel: &Parcel) -> Module {
    let label = parcel.label.clone();
    let module = parcel_url(inv_id, label.sha256);
    let all_features = label.feature.unwrap_or_default();
    let features = all_features
        .get("wagi")
        .map(|f| f.clone())
        .unwrap_or_default();
    let entrypoint = features.get("entrypoint").map(|s| s.clone());
    let bindle_server = features.get("bindle_server").map(|s| s.clone());
    let route = features
        .get("route")
        .map(|s| s.clone())
        .unwrap_or_else(|| "/".to_owned());
    let host = features.get("host").map(|s| s.clone());
    let allowed_hosts = features
        .get("allowed_hosts")
        .map(|ah| ah.split(",").map(|v| v.to_owned()).collect())
        .or_else(|| Some(vec![]));
    Module {
        module,
        entrypoint,
        bindle_server,
        route,
        host,
        allowed_hosts,
        volumes: None,
        environment: None,
    }
}

fn group_members(invoice: &Invoice, name: &str) -> Vec<Parcel> {
    invoice
        .parcel
        .clone()
        .unwrap_or_default()
        .iter()
        .filter(|p| p.member_of(name))
        .map(|p| p.clone())
        .collect()
}

fn is_file(parcel: &Parcel) -> bool {
    let wagi_key = "wagi".to_owned();
    let file_key = "file".to_owned();
    parcel
        .label
        .clone()
        .feature
        .and_then(|f| {
            f.clone()
                .get(&wagi_key)
                .and_then(|g| match g.get(&file_key) {
                    Some(v) => Some("true" == v),
                    None => Some(false),
                })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod test {
    use bindle::{BindleSpec, Condition, Group, Id, Invoice, Label, Parcel};
    use std::{collections::BTreeMap, convert::TryInto};

    use crate::runtime::bindle::{top_modules, WASM_MEDIA_TYPE};

    #[test]
    fn test_top_modules() {
        let inv = Invoice {
            bindle_version: "v1".to_owned(),
            yanked: None,
            signature: None,
            annotations: None,
            bindle: BindleSpec {
                id: "drink/1.2.3"
                    .to_owned()
                    .try_into()
                    .expect("This should parse"),
                description: None,
                authors: None,
            },
            group: Some(vec![Group {
                name: "coffee".to_owned(),
                required: None,
                satisfied_by: None,
            }]),
            parcel: Some(vec![
                Parcel {
                    label: Label {
                        sha256: "yubbadubbadoo".to_owned(),
                        name: "mocha-java".to_owned(),
                        media_type: WASM_MEDIA_TYPE.to_owned(),
                        size: 1234,
                        annotations: None,
                        feature: None,
                    },
                    conditions: Some(Condition {
                        member_of: Some(vec!["coffee".to_owned()]),
                        requires: None,
                    }),
                },
                Parcel {
                    label: Label {
                        sha256: "abc123".to_owned(),
                        name: "yirgacheffe".to_owned(),
                        media_type: WASM_MEDIA_TYPE.to_owned(),
                        size: 1234,
                        annotations: None,
                        feature: None,
                    },
                    conditions: Some(Condition {
                        member_of: Some(vec!["coffee".to_owned()]),
                        requires: None,
                    }),
                },
                Parcel {
                    label: Label {
                        sha256: "yubbadubbadoonow".to_owned(),
                        name: "water".to_owned(),
                        media_type: WASM_MEDIA_TYPE.to_owned(),
                        size: 1234,
                        annotations: None,
                        feature: None,
                    },
                    conditions: Some(Condition {
                        member_of: None,
                        requires: None,
                    }),
                },
            ]),
        };

        let res = top_modules(&inv);
        assert_eq!(res.len(), 1);
        assert_eq!(
            res.get(0).expect("first item").label.name,
            "water".to_owned()
        );
    }

    #[test]
    fn test_is_file() {
        let mut p = Parcel {
            label: Label {
                sha256: "yubbadubbadoonow".to_owned(),
                name: "water".to_owned(),
                media_type: WASM_MEDIA_TYPE.to_owned(),
                size: 1234,
                annotations: None,
                feature: None,
            },
            conditions: Some(Condition {
                member_of: None,
                requires: None,
            }),
        };
        assert!(!super::is_file(&p));

        let mut features = BTreeMap::new();
        let mut wagifeatures = BTreeMap::new();
        wagifeatures.insert("file".to_owned(), "true".to_owned());
        features.insert("wagi".to_owned(), wagifeatures);
        p.label.feature = Some(features);
        assert!(super::is_file(&p));
    }

    #[test]
    fn test_group_members() {
        let inv = Invoice {
            bindle_version: "v1".to_owned(),
            yanked: None,
            signature: None,
            annotations: None,
            bindle: BindleSpec {
                id: "drink/1.2.3"
                    .to_owned()
                    .try_into()
                    .expect("This should parse"),
                description: None,
                authors: None,
            },
            group: Some(vec![Group {
                name: "coffee".to_owned(),
                required: None,
                satisfied_by: None,
            }]),
            parcel: Some(vec![
                Parcel {
                    label: Label {
                        sha256: "yubbadubbadoo".to_owned(),
                        name: "mocha-java".to_owned(),
                        media_type: WASM_MEDIA_TYPE.to_owned(),
                        size: 1234,
                        annotations: None,
                        feature: None,
                    },
                    conditions: Some(Condition {
                        member_of: Some(vec!["coffee".to_owned()]),
                        requires: None,
                    }),
                },
                Parcel {
                    label: Label {
                        sha256: "abc123".to_owned(),
                        name: "yirgacheffe".to_owned(),
                        media_type: WASM_MEDIA_TYPE.to_owned(),
                        size: 1234,
                        annotations: None,
                        feature: None,
                    },
                    conditions: Some(Condition {
                        member_of: Some(vec!["coffee".to_owned()]),
                        requires: None,
                    }),
                },
                Parcel {
                    label: Label {
                        sha256: "yubbadubbadoonow".to_owned(),
                        name: "water".to_owned(),
                        media_type: WASM_MEDIA_TYPE.to_owned(),
                        size: 1234,
                        annotations: None,
                        feature: None,
                    },
                    conditions: Some(Condition {
                        member_of: None,
                        requires: None,
                    }),
                },
            ]),
        };

        let members = super::group_members(&inv, "coffee");
        assert_eq!(2, members.len());
    }
}
