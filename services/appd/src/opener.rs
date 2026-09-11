use alloc::string::String;
use alloc::vec::Vec;
use bexos_opener_store::{HandlerRegistration, MemoryOpenerRegistry, OpenerScope};

use crate::manifest::{Manifest, Process};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenerBinding {
    pub channel: u64,
    pub package: String,
    pub uid: u64,
    pub system: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenerRequest {
    pub caller_package: String,
    pub uid: u64,
    pub system: bool,
    pub kind: OpenerRequestKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpenerRequestKind {
    Url {
        url: String,
    },
    File {
        file: u64,
        mime_type: String,
    },
    App {
        package: String,
        arguments: Vec<String>,
    },
    Interface {
        name: String,
        server_channel: u64,
    },
    SetDefault {
        key: String,
        package: String,
    },
}

pub fn register_manifest_openers(
    registry: &mut MemoryOpenerRegistry,
    scope: OpenerScope,
    manifest: &Manifest,
    domains_verified: bool,
) {
    for process in &manifest.processes {
        if let Some(registration) = registration_from_process(manifest, process, domains_verified) {
            let _ = registry.register(scope, registration);
        }
    }
}

fn registration_from_process(
    manifest: &Manifest,
    process: &Process,
    domains_verified: bool,
) -> Option<HandlerRegistration> {
    let mut registration = HandlerRegistration {
        package: manifest.package_name.clone(),
        process: process.name.clone(),
        schemes: Vec::new(),
        domains: Vec::new(),
        mime_types: Vec::new(),
        interfaces: Vec::new(),
        domains_verified,
    };
    for filter in &process.handles {
        registration.schemes.extend(filter.schemes.iter().cloned());
        registration.domains.extend(filter.domains.iter().cloned());
        registration
            .mime_types
            .extend(filter.mime_types.iter().cloned());
        registration
            .interfaces
            .extend(filter.provides_interfaces.iter().cloned());
    }
    dedupe(&mut registration.schemes);
    dedupe(&mut registration.domains);
    dedupe(&mut registration.mime_types);
    dedupe(&mut registration.interfaces);
    if registration.schemes.is_empty()
        && registration.domains.is_empty()
        && registration.mime_types.is_empty()
        && registration.interfaces.is_empty()
    {
        None
    } else {
        Some(registration)
    }
}

fn dedupe(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}
