use alloc::vec::Vec;

use bexos_network_extension_abi::Hook;
use bexos_userspace::{Channel, Memory};
use net_fidl::*;
use sha2::{Digest, Sha256};

use crate::builtins::{FIREWALL_NAME, firewall, firewall_with_config, nat};
use crate::extensions::{Extension, FailurePolicy};
use crate::migration::Runtime;
use crate::switch::SwitchError;

pub fn handle(
    runtime: &mut Runtime,
    channel: Channel,
    ordinal: u64,
    request: &[u8],
    handles: &[HandleRef],
) {
    match ordinal {
        1 => {
            let result = SwitchExtensionControllerInstallRequest::decode(request, handles)
                .map_err(|_| SwitchError::InvalidExtension)
                .and_then(|request| install(runtime, request));
            let (status, active_generation) = match result {
                Ok(generation) => (Status::Ok, generation),
                Err(error) => (status(error), active_generation(runtime)),
            };
            reply(
                channel,
                &SwitchExtensionControllerInstallResponse {
                    status,
                    active_generation,
                },
            );
        }
        2 => {
            let result = SwitchExtensionControllerRemoveRequest::decode(request, handles)
                .map_err(|_| SwitchError::InvalidExtension)
                .and_then(|request| remove(runtime, request.name));
            let (status, active_generation) = match result {
                Ok(generation) => (Status::Ok, generation),
                Err(error) => (status(error), active_generation(runtime)),
            };
            reply(
                channel,
                &SwitchExtensionControllerRemoveResponse {
                    status,
                    active_generation,
                },
            );
        }
        3 => {
            let _ = SwitchExtensionControllerListRequest::decode(request, handles);
            let extensions = runtime
                .data_plane
                .extensions
                .iter()
                .map(|extension| NetworkExtensionStatus {
                    name: extension.name.as_str(),
                    kind: if extension.hook == Hook::Firewall {
                        NetworkExtensionKind::Firewall
                    } else {
                        NetworkExtensionKind::Nat
                    },
                    generation: extension.generation,
                    digest: extension.digest,
                    faulted: extension.faulted,
                    batches: extension.counters.batches,
                    packets: extension.counters.packets,
                    passed: extension.counters.passed,
                    dropped: extension.counters.dropped,
                    rewritten: extension.counters.rewritten,
                    redirected: extension.counters.redirected,
                    traps: extension.counters.traps,
                })
                .collect::<Vec<_>>();
            reply(
                channel,
                &SwitchExtensionControllerListResponse {
                    status: Status::Ok,
                    extensions: WireVector::from_slice(&extensions),
                },
            );
        }
        _ => {}
    }
}

fn install(
    runtime: &mut Runtime,
    request: SwitchExtensionControllerInstallRequest<'_>,
) -> Result<u64, SwitchError> {
    let hook = match request.hook {
        NetworkExtensionHook::Bridge => Hook::Bridge,
        NetworkExtensionHook::PreRouting => Hook::PreRouting,
        NetworkExtensionHook::Firewall => Hook::Firewall,
        NetworkExtensionHook::PostRouting => Hook::PostRouting,
    };
    if (request.kind == NetworkExtensionKind::Firewall) != (hook == Hook::Firewall) {
        if request.module.raw != 0 {
            let _ = Memory::close(request.module.raw);
        }
        return Err(SwitchError::InvalidExtension);
    }
    if request.module.raw == 0 {
        if request.kind == NetworkExtensionKind::Firewall {
            runtime
                .data_plane
                .extensions
                .replace_hook(firewall_with_config(request.config, request.generation)?)?;
        } else {
            runtime
                .data_plane
                .extensions
                .replace_nat_pair(nat(request.config, request.generation)?)?;
        }
        return Ok(request.generation);
    }
    if request.module_length == 0 || request.module_length > 4 << 20 {
        let _ = Memory::close(request.module.raw);
        return Err(SwitchError::InvalidExtension);
    }
    let rounded = request
        .module_length
        .checked_add(4095)
        .ok_or(SwitchError::InvalidExtension)?
        & !4095;
    let address =
        Memory::map(request.module.raw, rounded, 2).map_err(|_| SwitchError::InvalidExtension)?;
    let bytes = unsafe {
        core::slice::from_raw_parts(address as *const u8, request.module_length as usize).to_vec()
    };
    let _ = Memory::unmap(address, rounded);
    let _ = Memory::close(request.module.raw);
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    if digest != request.expected_digest {
        return Err(SwitchError::InvalidExtension);
    }
    let policy = if request.fail_open {
        FailurePolicy::Open
    } else {
        FailurePolicy::Closed
    };
    if request.kind == NetworkExtensionKind::Firewall {
        let extension = Extension::compile(
            request.name,
            hook,
            policy,
            digest,
            &bytes,
            request.config,
            request.generation,
        )?;
        runtime.data_plane.extensions.replace_hook(extension)?;
    } else {
        let (pre_name, post_name) = nat_names(request.name);
        let pre = Extension::compile(
            &pre_name,
            Hook::PreRouting,
            policy,
            digest,
            &bytes,
            request.config,
            request.generation,
        )?;
        let post = Extension::compile(
            &post_name,
            Hook::PostRouting,
            policy,
            digest,
            &bytes,
            request.config,
            request.generation,
        )?;
        runtime
            .data_plane
            .extensions
            .replace_nat_pair([pre, post])?;
    }
    Ok(request.generation)
}

fn nat_names(requested: &str) -> (alloc::string::String, alloc::string::String) {
    if let Some(base) = requested.strip_suffix(".pre-routing") {
        return (requested.into(), alloc::format!("{base}.post-routing"));
    }
    if let Some(base) = requested.strip_suffix(".post-routing") {
        return (alloc::format!("{base}.pre-routing"), requested.into());
    }
    (
        alloc::format!("{requested}.pre-routing"),
        alloc::format!("{requested}.post-routing"),
    )
}

fn remove(runtime: &mut Runtime, name: &str) -> Result<u64, SwitchError> {
    if name == FIREWALL_NAME {
        return Err(SwitchError::InUse);
    }
    let hook = runtime
        .data_plane
        .extensions
        .iter()
        .find(|extension| extension.name == name)
        .map(|extension| extension.hook)
        .ok_or(SwitchError::NotFound)?;
    if matches!(hook, Hook::PreRouting | Hook::PostRouting) {
        let removed = runtime.data_plane.extensions.remove_nat_pair(name)?;
        return Ok(removed
            .iter()
            .map(|extension| extension.generation)
            .max()
            .unwrap_or(0)
            .saturating_add(1));
    }
    let removed = runtime.data_plane.extensions.remove(name)?;
    let generation = removed.generation.saturating_add(1);
    if removed.hook == Hook::Firewall {
        runtime
            .data_plane
            .extensions
            .replace_hook(firewall(generation)?)?;
    }
    Ok(generation)
}

fn active_generation(runtime: &Runtime) -> u64 {
    runtime
        .data_plane
        .extensions
        .iter()
        .map(|extension| extension.generation)
        .max()
        .unwrap_or(0)
}

fn status(error: SwitchError) -> Status {
    match error {
        SwitchError::NotFound => Status::ErrNotFound,
        SwitchError::QueueFull => Status::ErrResourceExhausted,
        SwitchError::InUse => Status::ErrAccessDenied,
        _ => Status::ErrInvalidArgs,
    }
}

fn reply<T: FidlEncode>(channel: Channel, response: &T) {
    let mut bytes = alloc::vec![0; 4096];
    if let Ok(encoded) = response.encode(&mut bytes, &mut []) {
        let _ = channel.send(&bytes[..encoded.bytes], &[]);
    }
}
