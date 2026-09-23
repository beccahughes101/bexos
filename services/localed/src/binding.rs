use crate::service::Runtime;
use bexos_userspace::{Memory, Message, service_binding::ServiceBinding};
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Client {
    pub channel: u64,
    pub uid: u64,
    pub startup: bool,
    pub methods: Vec<u64>,
}
impl Client {
    pub fn from_binding(channel: u64, b: &ServiceBinding) -> Option<Self> {
        let uid = b.caller_uid?;
        let startup = b.service == "bexos.locale.LocaleStartup"
            && b.protocol == "LocaleStartup"
            && b.capability == "Startup"
            && uid == 0
            && b.caller_package.as_deref() == Some("bexos.platform.appd")
            && b.method_ordinals == [1];
        let public = b.service == "bexos.locale.LocaleProvider"
            && b.protocol == "LocaleProvider"
            && b.capability == "Public"
            && b.caller_package.is_some()
            && b.method_ordinals.iter().all(|m| matches!(m, 1 | 2 | 3));
        (startup || public).then(|| Self {
            channel,
            uid,
            startup,
            methods: b.method_ordinals.clone(),
        })
    }
}
pub fn accept(runtime: &mut Runtime, message: Message) {
    if runtime.clients.len() < 256 && message.handles.len() == 1 {
        if let Some(client) = core::str::from_utf8(&message.bytes)
            .ok()
            .and_then(ServiceBinding::parse)
            .and_then(|b| Client::from_binding(message.handles[0], &b))
        {
            runtime.clients.push(client);
            return;
        }
    }
    for h in message.handles {
        let _ = Memory::close(h);
    }
}
