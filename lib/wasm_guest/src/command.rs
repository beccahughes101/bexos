//! Shared capability-based command transport for WASM applications.
use crate::{
    bexos::wasm::kernel,
    wasi::filesystem::{
        preopens,
        types::{DescriptorFlags, OpenFlags, PathFlags},
    },
};
use app_opener_fidl as opener_fidl;
use opener_fidl::{FidlDecode, FidlEncode, HandleRef};
pub struct Resource(pub u32);
impl Drop for Resource {
    fn drop(&mut self) {
        let _ = kernel::resource_close(self.0);
    }
}
pub async fn receive(channel: u32) -> Result<kernel::Message, String> {
    loop {
        match kernel::channel_read_checked(channel, 32768, 4) {
            Ok(message) => return Ok(message),
            Err(kernel::StreamError::WouldBlock) => tokio::task::yield_now().await,
            Err(_) => return Err("command endpoint disconnected".into()),
        }
    }
}
pub fn send<T: FidlEncode>(channel: u32, ordinal: u64, request: &T) -> Result<(), String> {
    let mut data = vec![0; 32768];
    let mut handles = [HandleRef { raw: 0 }; 4];
    let size = request
        .encode(&mut data[8..], &mut handles)
        .map_err(|_| "command arguments exceed transport limits")?;
    data[..8].copy_from_slice(&ordinal.to_le_bytes());
    data.truncate(8 + size.bytes);
    kernel::channel_write(
        channel,
        &kernel::Message {
            data,
            resources: handles[..size.handles]
                .iter()
                .map(|h| h.raw as u32)
                .collect(),
        },
    )
    .map_err(|_| "command request could not be sent".into())
}
static OPENER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub async fn bind() -> Result<Resource, String> {
    let _lock = OPENER.lock().await;
    let opener = kernel::resource_find("Opener").ok_or("Opener capability was not granted")?;
    let (a, b) = kernel::channel_pair().map_err(|_| "command channel limit")?;
    let server = Resource(a);
    let client = Resource(b);
    send(
        opener,
        6,
        &opener_fidl::OpenerBindCommandLauncherRequest {
            launcher: HandleRef {
                raw: server.0.into(),
            },
        },
    )?;
    let reply = receive(opener).await?;
    let response = opener_fidl::OpenerBindCommandLauncherResponse::decode(&reply.data, &[])
        .map_err(|_| "invalid launcher binding reply")?;
    if response.status != opener_fidl::OpenerStatus::Ok {
        return Err("command launch permission denied".into());
    }
    Ok(client)
}
pub fn cwd(path: &std::path::Path) -> Result<Resource, String> {
    let path = path.to_str().ok_or("CWD is not UTF-8")?;
    let mut directories = preopens::get_directories();
    directories.sort_by_key(|(_, name)| std::cmp::Reverse(name.len()));
    for (directory, name) in directories {
        // A preopen already names this directory. Delegate its capability
        // directly; filesystem services need not accept a synthetic "." open.
        if path == name {
            return kernel::export_directory(&directory)
                .map(Resource)
                .map_err(|_| "CWD capability cannot be delegated".into());
        }
        let relative = path.strip_prefix(&format!("{}/", name.trim_end_matches('/')));
        if let Some(relative) = relative {
            let flags = directory
                .get_flags()
                .map_err(|e| format!("CWD flags: {e:?}"))?
                & (DescriptorFlags::READ
                    | DescriptorFlags::WRITE
                    | DescriptorFlags::MUTATE_DIRECTORY);
            let directory = directory
                .open_at(
                    PathFlags::SYMLINK_FOLLOW,
                    relative,
                    OpenFlags::DIRECTORY,
                    flags,
                )
                .map_err(|e| format!("CWD: {e:?}"))?;
            return kernel::export_directory(&directory)
                .map(Resource)
                .map_err(|_| "CWD capability cannot be delegated".into());
        }
    }
    Err("CWD is outside the granted namespace".into())
}
