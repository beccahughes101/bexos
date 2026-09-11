//! Retained service endpoints and their granted method sets.
use crate::migration::Runtime;
use bexos_lazy_service::{GuardedServiceEndpoint, accept_control_binding};
use bexos_userspace::{
    Channel, Memory,
    service_binding::{BoundServiceEndpoint, ServiceBinding},
};

pub(crate) fn accept_initial(runtime: &mut Runtime, grants: &[bexos_userspace::ServiceGrant]) {
    for grant in grants {
        if runtime.clients.len() >= 128 || !valid_keychain_binding_grant(grant) {
            if grant.endpoint != 0 {
                let _ = Memory::close(grant.endpoint);
            }
            continue;
        }
        let endpoint = BoundServiceEndpoint::new_with_protocol(
            Channel(grant.endpoint),
            grant.method_ordinals.clone(),
            &grant.protocol,
        );
        let guard = runtime.lazy.track_connection();
        runtime
            .clients
            .push(GuardedServiceEndpoint::from_existing(endpoint, guard));
    }
}

pub(crate) fn poll(runtime: &mut Runtime) {
    if let Ok(message) = runtime.control.try_recv() {
        if handle_lazy_control(runtime, &message) {
            return;
        }
        if core::str::from_utf8(&message.bytes)
            .ok()
            .and_then(ServiceBinding::parse)
            .is_some()
        {
            if runtime.clients.len() >= 128 {
                for handle in message.handles {
                    let _ = Memory::close(handle);
                }
            } else if let Ok(endpoint) =
                accept_control_binding(&runtime.lazy, &message, valid_keychain_binding)
            {
                runtime.clients.push(endpoint);
            }
        } else {
            crate::runtime::handle_request(runtime, runtime.control, message);
        }
    }
    let clients = core::mem::take(&mut runtime.clients);
    for client in clients {
        match client.channel().try_recv() {
            Ok(message) => {
                let ordinal = message
                    .bytes
                    .get(..8)
                    .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()));
                if ordinal.is_some_and(|ordinal| client.allows(ordinal)) {
                    crate::runtime::handle_request(runtime, client.channel(), message);
                } else {
                    for handle in message.handles {
                        let _ = Memory::close(handle);
                    }
                    let _ = Memory::close(client.channel().0);
                    continue;
                }
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                let _ = Memory::close(client.channel().0);
                continue;
            }
            Err(_) => {}
        }
        runtime.clients.push(client);
    }
}

fn valid_keychain_binding(binding: &ServiceBinding) -> bool {
    binding.service == "bexos.security.Keychain"
        && binding.protocol_is("Keychain")
        && binding.method_ordinals.len() <= 7
        && binding
            .method_ordinals
            .iter()
            .all(|method| (1..=7).contains(method))
}

fn valid_keychain_binding_grant(grant: &bexos_userspace::ServiceGrant) -> bool {
    grant.service == "bexos.security.Keychain"
        && grant.protocol == "Keychain"
        && grant.method_ordinals.len() <= 7
        && grant
            .method_ordinals
            .iter()
            .all(|method| (1..=7).contains(method))
}

fn handle_lazy_control(runtime: &mut Runtime, message: &bexos_userspace::Message) -> bool {
    let Ok(text) = core::str::from_utf8(&message.bytes) else {
        return false;
    };
    if let Some(generation) = text
        .strip_prefix("bexos.lazy.idle.ok|")
        .and_then(|value| value.parse::<u64>().ok())
    {
        if runtime.idle_stop_requested == Some(generation)
            && runtime.service.prepare_stop()
            && matches!(
                runtime.lazy.acknowledge_stop(generation),
                bexos_lazy_service::StopDecision::Accepted { .. }
            )
        {
            bexos_userspace::exit();
        }
        runtime.idle_stop_requested = None;
        runtime.lazy.cancel_stop();
        return true;
    }
    if text.starts_with("bexos.lazy.idle.busy|") {
        runtime.idle_stop_requested = None;
        runtime.lazy.cancel_stop();
        return true;
    }
    false
}
