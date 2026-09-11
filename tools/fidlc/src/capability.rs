use std::collections::BTreeMap;

use crate::ir;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityGroup {
    pub protocol: String,
    pub capability: String,
    pub permission: Option<String>,
    pub methods: Vec<CapabilityMethod>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityMethod {
    pub name: String,
    pub ordinal: u64,
}

pub fn split_library(library: &ir::Library) -> Vec<CapabilityGroup> {
    let mut groups = Vec::new();

    for protocol in &library.protocols {
        let mut by_permission: BTreeMap<Option<String>, Vec<CapabilityMethod>> = BTreeMap::new();
        for method in &protocol.methods {
            by_permission
                .entry(method.permission.clone())
                .or_default()
                .push(CapabilityMethod {
                    name: method.name.clone(),
                    ordinal: method.ordinal,
                });
        }

        for (permission, methods) in by_permission {
            let capability = match &permission {
                Some(permission) => capability_name(permission),
                None => "Public".to_string(),
            };
            groups.push(CapabilityGroup {
                protocol: protocol.name.clone(),
                capability,
                permission,
                methods,
            });
        }
    }

    groups
}

pub fn capability_name(permission: &str) -> String {
    let mut out = String::new();
    let mut capitalize = true;

    for ch in permission.chars() {
        if ch.is_ascii_alphanumeric() {
            if capitalize {
                out.push(ch.to_ascii_uppercase());
                capitalize = false;
            } else {
                out.push(ch.to_ascii_lowercase());
            }
        } else {
            capitalize = true;
        }
    }

    if out.is_empty() {
        "Permission".to_string()
    } else if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        format!("Permission{out}")
    } else {
        out
    }
}
