use alloc::vec::Vec;
use bexos_userspace::service_binding::ServiceBinding;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Client {
    pub channel: u64,
    pub uid: u64,
    pub methods: Vec<u64>,
    pub install: bool,
}

impl Client {
    pub fn from_binding(channel: u64, binding: &ServiceBinding) -> Option<Self> {
        if binding.service != "bexos.fonts.FontProvider" || binding.protocol != "FontProvider" {
            return None;
        }
        let uid = binding.caller_uid?;
        let install = binding.capability == "InstallUserFont" && binding.method_ordinals == [3];
        let public = binding.capability == "Public"
            && binding
                .method_ordinals
                .iter()
                .all(|method| matches!(method, 1 | 2));
        (install || public).then(|| Self {
            channel,
            uid,
            methods: binding.method_ordinals.clone(),
            install,
        })
    }
}
