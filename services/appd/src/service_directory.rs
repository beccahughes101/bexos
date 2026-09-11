use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use app_service_directory_fidl as service_directory;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

#[derive(Clone)]
pub struct ServiceDirectoryBinding {
    pub channel: u64,
    pub package: String,
    pub uid: u64,
    pub system: bool,
    pub shell: bool,
}

pub fn encode_service_directory_binding(binding: &ServiceDirectoryBinding) -> Vec<u8> {
    let mut w = Encoder::new();
    w.word(0x5344495232);
    w.word(binding.channel);
    w.text(&binding.package);
    w.word(binding.uid);
    w.word(binding.system as u64);
    w.word(binding.shell as u64);
    w.finish()
}

pub fn decode_service_directory_binding(bytes: &[u8]) -> Result<ServiceDirectoryBinding, Error> {
    let mut r = Decoder::new(bytes);
    let first = r.word()?;
    let versioned = first == 0x5344495232;
    let binding = ServiceDirectoryBinding {
        channel: if versioned { r.word()? } else { first },
        package: r.text(128)?.to_string(),
        uid: r.word()?,
        system: r.flag()?,
        shell: if versioned { r.flag()? } else { false },
    };
    r.finish()?;
    if binding.channel == 0 || binding.package.is_empty() {
        return Err(Error::InvalidData);
    }
    Ok(binding)
}

pub fn status_from_lifecycle(
    status: app_lifecycle_fidl::AppLifecycleStatus,
) -> service_directory::ServiceDirectoryStatus {
    match status {
        app_lifecycle_fidl::AppLifecycleStatus::Ok => service_directory::ServiceDirectoryStatus::Ok,
        app_lifecycle_fidl::AppLifecycleStatus::NotFound => {
            service_directory::ServiceDirectoryStatus::NotFound
        }
        app_lifecycle_fidl::AppLifecycleStatus::LaunchFailed => {
            service_directory::ServiceDirectoryStatus::LaunchFailed
        }
        app_lifecycle_fidl::AppLifecycleStatus::AccessDenied => {
            service_directory::ServiceDirectoryStatus::AccessDenied
        }
        _ => service_directory::ServiceDirectoryStatus::InvalidArgs,
    }
}
