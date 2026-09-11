use alloc::{string::String, vec::Vec};
use bexos_userspace::{Channel, Memory, service_binding::ServiceBinding};
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Client {
    pub channel: u64,
    pub package: String,
    pub package_key: String,
    pub uid: u64,
    pub methods: Vec<u64>,
    pub admin: bool,
    pub manage: bool,
}
impl Client {
    pub fn from_binding(channel: u64, b: &ServiceBinding) -> Option<Self> {
        let package = b.caller_package.clone()?;
        let uid = b.caller_uid?;
        let package_key = b
            .permission_values
            .iter()
            .find_map(|v| v.strip_prefix("caller-package-key:"))
            .unwrap_or(&package)
            .into();
        let admin =
            b.protocol == "PreferencesAdmin" && package == "bexos.platform.appd" && uid == 0;
        if !admin
            && (b.service != "bexos.preferences.UserPreferences" || b.protocol != "UserPreferences")
        {
            return None;
        }
        let manage = !admin && b.capability == "ManageUserPreferences" && b.method_ordinals == [4];
        if !admin
            && !manage
            && (b.capability != "Public"
                || b.method_ordinals.iter().any(|m| !matches!(m, 1 | 2 | 3)))
        {
            return None;
        }
        Some(Self {
            channel,
            package,
            package_key,
            uid,
            methods: b.method_ordinals.clone(),
            admin,
            manage,
        })
    }
}
pub fn accept(
    control: Channel,
    message: bexos_userspace::Message,
    clients: &mut Vec<Client>,
) -> bool {
    let binding = core::str::from_utf8(&message.bytes)
        .ok()
        .and_then(ServiceBinding::parse);
    if let Some(binding) = binding {
        if message.handles.len() == 1 && clients.len() < 256 {
            if let Some(client) = Client::from_binding(message.handles[0], &binding) {
                clients.push(client);
                return true;
            }
        }
        for h in message.handles {
            let _ = Memory::close(h);
        }
        return true;
    }
    for handle in message.handles {
        let _ = Memory::close(handle);
    }
    let _ = control;
    false
}
