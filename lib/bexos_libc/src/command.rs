//! Command startup stdio and environment, separate from service boot defaults.
use super::*;
static ENVIRONMENT: Locked<Vec<(Vec<u8>, Vec<u8>)>> = Locked::new(Vec::new());
pub(super) fn install(startup: &bexos_userspace::Startup) {
    let Ok(Some(options)) = bexos_userspace::command::from_startup(startup) else {
        return;
    };
    for (fd, raw) in startup.resources[..3].iter().copied().enumerate() {
        let Ok((kind, rights)) = bexos_userspace::Memory::object_info(raw) else {
            continue;
        };
        // libc and the embedding WASM runtime own separate references.
        let Ok(raw) = bexos_userspace::Memory::duplicate(raw, rights) else {
            continue;
        };
        let kind = match kind {
            kernel_fidl::ObjectType::Socket => FdKind::TcpStream {
                socket: raw,
                control: 0,
            },
            kernel_fidl::ObjectType::Channel => FdKind::File { channel: raw },
            _ => {
                let _ = bexos_userspace::Memory::close(raw);
                continue;
            }
        };
        FD_TABLE.with(|table| {
            table.entries[fd] = FdEntry {
                kind,
                flags: 0,
                refs: 1,
            }
        });
    }
    ENVIRONMENT.with(|entries| {
        *entries = options
            .environment
            .into_iter()
            .map(|(name, value)| {
                let mut value = value.into_bytes();
                value.push(0);
                (name.into_bytes(), value)
            })
            .collect();
    });
}
pub(super) unsafe fn getenv(name: *const c_char) -> *mut c_char {
    if name.is_null() {
        return ptr::null_mut();
    }
    let name = unsafe { core::ffi::CStr::from_ptr(name) }.to_bytes();
    ENVIRONMENT.with(|entries| {
        entries
            .iter_mut()
            .find(|(key, _)| key == name)
            .map_or(ptr::null_mut(), |(_, value)| value.as_mut_ptr().cast())
    })
}
