use crate::runtime::{Client, Runtime};
use bexos_userspace::{Memory, Message, service_binding::ServiceBinding};

impl Client {
    fn from_binding(channel: u64, binding: &ServiceBinding) -> Option<Self> {
        (binding.service == "bexos.container.ContainerManager"
            && binding.protocol == "ContainerManager"
            && binding.capability == "Public"
            && binding.caller_package.is_some()
            && binding.caller_uid.is_some()
            && binding.method_ordinals.iter().all(|m| matches!(m, 1..=6)))
        .then(|| Self {
            channel,
            methods: binding.method_ordinals.clone(),
        })
    }
}

pub fn accept(runtime: &mut Runtime, message: Message) {
    if runtime.clients.len() < 256 && message.handles.len() == 1 {
        if let Some(client) = core::str::from_utf8(&message.bytes)
            .ok()
            .and_then(ServiceBinding::parse)
            .and_then(|binding| Client::from_binding(message.handles[0], &binding))
        {
            runtime.clients.push(client);
            return;
        }
    }
    for handle in message.handles {
        let _ = Memory::close(handle);
    }
}
