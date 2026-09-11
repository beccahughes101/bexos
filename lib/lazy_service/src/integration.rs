use bexos_userspace::{
    Channel, Memory,
    service_binding::{BoundServiceEndpoint, ServiceBinding},
};

use crate::controller::{ConnectionGuard, LazyServiceController};

pub struct GuardedServiceEndpoint {
    pub endpoint: BoundServiceEndpoint,
    _guard: ConnectionGuard,
}

impl GuardedServiceEndpoint {
    pub fn from_existing(endpoint: BoundServiceEndpoint, guard: ConnectionGuard) -> Self {
        Self {
            endpoint,
            _guard: guard,
        }
    }

    pub fn channel(&self) -> Channel {
        self.endpoint.channel
    }

    pub fn allows(&self, ordinal: u64) -> bool {
        self.endpoint.allows(ordinal)
    }

    pub fn into_parts(self) -> (BoundServiceEndpoint, ConnectionGuard) {
        (self.endpoint, self._guard)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingAcceptError {
    InvalidMetadata,
    InvalidHandleCount,
    Rejected,
}

pub fn accept_control_binding<F>(
    controller: &LazyServiceController,
    message: &bexos_userspace::Message,
    mut accept: F,
) -> Result<GuardedServiceEndpoint, BindingAcceptError>
where
    F: FnMut(&ServiceBinding) -> bool,
{
    let binding = core::str::from_utf8(&message.bytes)
        .ok()
        .and_then(ServiceBinding::parse)
        .ok_or(BindingAcceptError::InvalidMetadata)?;
    if message.handles.len() != 1 {
        close_handles(&message.handles);
        return Err(BindingAcceptError::InvalidHandleCount);
    }
    if !accept(&binding) {
        close_handles(&message.handles);
        return Err(BindingAcceptError::Rejected);
    }
    let guard = controller.track_connection();
    Ok(GuardedServiceEndpoint {
        endpoint: BoundServiceEndpoint::new_with_protocol(
            Channel(message.handles[0]),
            binding.method_ordinals,
            &binding.protocol,
        ),
        _guard: guard,
    })
}

fn close_handles(handles: &[u64]) {
    for handle in handles {
        let _ = Memory::close(*handle);
    }
}
