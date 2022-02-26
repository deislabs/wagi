use std::{collections::{HashMap, HashSet}, iter::FromIterator};

use bindle::{Invoice, Parcel};

// TODO: this file is a bit of a cop-out but will be useful during
// the transition.  Find better homes for these things!

pub const WASM_MEDIA_TYPE: &str = "application/wasm";

pub struct InvoiceUnderstander {
    invoice: Invoice,
    group_dependency_map: HashMap<String, Vec<Parcel>>,
}

impl InvoiceUnderstander {
    pub fn new(invoice: &Invoice) -> Self {
        let group_dependency_map = build_full_memberships(invoice);
        Self {
            invoice: invoice.clone(),
            group_dependency_map,
        }
    }

    pub fn id(&self) -> bindle::Id {
        self.invoice.bindle.id.clone()
    }

    // America's next...
    pub fn top_modules(&self) -> Vec<Parcel> {
        self.invoice
            .parcel
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

    pub fn classify_parcel(&self, parcel: &Parcel) -> Option<InterestingParcel> {
        // Currently only handlers but we have talked of scheduled tasks etc.
        parcel.label.feature.as_ref().and_then(|features| {
            features.get("wagi").and_then(|wagi_features| {
                match wagi_features.get("route") {
                    Some(route) => {
                        let handler_info = WagiHandlerInfo {
                            invoice_id: self.id(),
                            parcel: parcel.clone(),
                            route: route.to_owned(),
                            entrypoint: wagi_features.get("entrypoint").map(|s| s.to_owned()),
                            allowed_hosts: wagi_features.get("allowed_hosts").map(|h| parse_csv(h)),
                            argv: wagi_features.get("argv").map(|s| s.to_owned()),
                            required_parcels: parcels_required_for(parcel, &self.group_dependency_map),
                        };
                        Some(InterestingParcel::WagiHandler(handler_info))
                    },
                    None => None,
                }
            })
        })
    }

    pub fn parse_wagi_handlers(&self) -> Vec<WagiHandlerInfo> {
        self
            .top_modules().iter()
            .filter_map(|parcel| self.classify_parcel(parcel))
            .map(|parcel| match parcel {    // If there are other cases of InterestingParcel this may need to become a filter_map, but right now that makes Clippy mad
                InterestingParcel::WagiHandler(h) => h,
            })
            .collect()
    }
}

pub enum InterestingParcel {
    WagiHandler(WagiHandlerInfo),
}

#[derive(Clone)]
pub struct WagiHandlerInfo {
    pub invoice_id: bindle::Id,
    pub parcel: Parcel,
    pub route: String,
    pub entrypoint: Option<String>,
    pub allowed_hosts: Option<Vec<String>>,
    pub required_parcels: Vec<Parcel>,
    pub argv: Option<String>,
}

impl WagiHandlerInfo {
    pub fn asset_parcels(&self) -> Vec<Parcel> {
        self.required_parcels.iter().filter(|p| is_file(p)).cloned().collect()
    }
}

const NO_PARCELS: Vec<Parcel> = vec![];

pub fn is_file(parcel: &Parcel) -> bool {
    parcel.label.feature.as_ref().and_then(|features| {
        features.get("wagi").map(|wagi_features| {
            match wagi_features.get("file") {
                Some(s) => s == "true",
                _ => false,
            }
        })
    }).unwrap_or(false)
}

pub fn parcels_required_for(parcel: &Parcel, full_dep_map: &HashMap<String, Vec<Parcel>>) -> Vec<Parcel> {
    let mut required = HashSet::new();
    for group in parcel.directly_requires() {
        required.extend(full_dep_map.get(&group).unwrap_or(&NO_PARCELS).iter().cloned());
    }
    Vec::from_iter(required)
}

fn build_direct_memberships(invoice: &Invoice) -> HashMap<String, Vec<Parcel>> {
    let mut direct_memberships: HashMap<String, Vec<Parcel>> = HashMap::new();
    for parcel in invoice.parcel.clone().unwrap_or_default() {
        if let Some(condition) = &parcel.conditions {
            if let Some(memberships) = &condition.member_of {
                for group in memberships {
                    if let Some(existing) = direct_memberships.get_mut(group) {
                        existing.push(parcel.clone());
                    } else {
                        direct_memberships.insert(group.to_owned(), vec![parcel.clone()]);
                    }
                }
            }
        }
    }
    direct_memberships
}

pub fn build_full_memberships(invoice: &Invoice) -> HashMap<String, Vec<Parcel>> {
    let direct_memberships = build_direct_memberships(invoice);
    let gg_deps = group_to_group_full_dependencies(&direct_memberships);
    let mut full_memberships = HashMap::new();

    for group in direct_memberships.keys() {
        let mut all_members = HashSet::new();
        for dep_group in gg_deps.get(group).unwrap() {
            all_members.extend(direct_memberships.get(dep_group).unwrap_or(&NO_PARCELS).iter().cloned());
        }
        full_memberships.insert(group.to_owned(), Vec::from_iter(all_members));
    }

    full_memberships
}

fn group_to_group_direct_dependencies(direct_memberships: &HashMap<String, Vec<Parcel>>) -> HashMap<String, Vec<String>> {
    let mut ggd = HashMap::new();
    for (group, members) in direct_memberships {
        let mut directs: Vec<_> = members.iter().flat_map(|parcel| parcel.directly_requires()).collect();
        directs.push(group.to_owned());
        ggd.insert(group.to_owned(), directs);
    }
    ggd
}

fn direct_deps_not_already_in_list(list: &[String], direct_dep_map: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut new_dds = vec![];
    for group in list {
        if let Some(child_groups) = direct_dep_map.get(group) {
            for new_one in child_groups.iter().filter(|cg| !list.contains(cg)) {
                new_dds.push(new_one.to_owned());
            }
        }
    }
    HashSet::<String>::from_iter(new_dds).into_iter().collect()
}

fn group_to_group_full_dependencies(direct_memberships: &HashMap<String, Vec<Parcel>>) -> HashMap<String, Vec<String>> {
    let mut ggd = HashMap::new();
    let direct_deps = group_to_group_direct_dependencies(direct_memberships);
    for (group, directs) in &direct_deps {
        let mut full = directs.clone();
        let mut unchecked = directs.clone();
        loop {
            let new_ones = direct_deps_not_already_in_list(&unchecked, &direct_deps);
            if new_ones.is_empty() {
                break;
            }
            unchecked = new_ones.clone();
            full.extend(new_ones);
        }
        ggd.insert(group.to_owned(), full);
    }
    ggd
}

trait ParcelUtils {
    fn directly_requires(&self) -> Vec<String>;
}

impl ParcelUtils for Parcel {
    fn directly_requires(&self) -> Vec<String> {
        match &self.conditions {
            Some(condition) => match &condition.requires {
                Some(groups) => groups.clone(),
                None => vec![],
            },
            None => vec![],
        }
    }
}

fn parse_csv(text: &str) -> Vec<String> {
    text.split(',').map(|v| v.to_owned()).collect()  // TODO: trim etc.?
}

#[cfg(test)]
mod test {
    use super::*;

    use bindle::{BindleSpec, Condition, Group, Label};
    use std::{collections::BTreeMap, convert::TryInto};

    #[test]
    fn test_top_modules() {
        let inv = InvoiceUnderstander::new(&Invoice {
            bindle_version: "v1".to_owned(),
            yanked: None,
            yanked_signature: None,
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
                        origin: None,
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
                        origin: None,
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
                        origin: None,
                    },
                    conditions: Some(Condition {
                        member_of: None,
                        requires: None,
                    }),
                },
            ]),
        });

        let res = inv.top_modules();
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
                origin: None,
            },
            conditions: Some(Condition {
                member_of: None,
                requires: None,
            }),
        };
        assert!(!is_file(&p));

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
            yanked_signature: None,
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
                        origin: None,
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
                        origin: None,
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
                        origin: None,
                    },
                    conditions: Some(Condition {
                        member_of: None,
                        requires: None,
                    }),
                },
            ]),
        };

        let membership_map = build_full_memberships(&inv);
        let members = membership_map.get("coffee").expect("there should have been a group called 'coffee'");
        assert_eq!(2, members.len());
    }
}
