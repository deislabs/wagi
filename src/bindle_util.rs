use std::{collections::{HashMap, HashSet}, iter::FromIterator};

use bindle::{Invoice, Parcel};

// TODO: this file is a bit of a cop-out but will be useful during
// the transition.  Find better homes for these things!

pub const WASM_MEDIA_TYPE: &str = "application/wasm";

// America's next...
pub fn top_modules(invoice: &Invoice) -> Vec<Parcel> {
    invoice.parcel
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

pub enum InterestingParcel {
    WagiHandler(WagiHandlerInfo),
}

pub struct WagiHandlerInfo {
    pub parcel: Parcel,
    pub route: String,
}

impl InterestingParcel {
    pub fn parcel(&self) -> &Parcel {
        match self {
            Self::WagiHandler(handler_info) => &handler_info.parcel,
        }
    }
}

const NO_PARCELS: Vec<Parcel> = vec![];

pub fn classify_parcel(parcel: &Parcel) -> Option<InterestingParcel> {
    // Currently only handlers but we have talked of scheduled tasks etc.
    parcel.label.feature.as_ref().and_then(|features| {
        features.get("wagi").and_then(|wagi_features| {
            match wagi_features.get("route") {
                Some(route) => {
                    let handler_info = WagiHandlerInfo {
                        parcel: parcel.clone(),
                        route: route.to_owned()
                    };
                    Some(InterestingParcel::WagiHandler(handler_info))
                },
                None => None,
            }
        })
    })
}

pub fn parcels_required_for(parcel: &Parcel, full_dep_map: &HashMap<String, Vec<Parcel>>) -> Vec<Parcel> {
    let mut required = HashSet::new();
    for group in parcel.directly_requires() {
        required.extend(full_dep_map.get(&group).unwrap_or(&NO_PARCELS).iter().map(|p| p.clone()));
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
            all_members.extend(direct_memberships.get(dep_group).unwrap_or(&NO_PARCELS).iter().map(|p| p.clone()));
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

fn direct_deps_not_already_in_list(list: &Vec<String>, direct_dep_map: &HashMap<String, Vec<String>>) -> Vec<String> {
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
