#![no_std]
extern crate alloc;

#[cfg(all(bexos_guest, target_arch = "aarch64"))]
core::arch::global_asm!(
    r#"
    .section .note.gnu.property, "a", %note
    .p2align 3
    .long 4
    .long 16
    .long 5
    .asciz "GNU"
    .p2align 3
    .long 0xc0000000
    .long 4
    .long 3
    .long 0
    .p2align 3
"#
);

mod arch;
pub mod command;
pub mod dynamic_link;
pub mod executor;
pub mod fs;
pub mod ipc;
pub mod memory;
pub mod preferences;
pub mod startup;
mod startup_compat;
pub mod syscall;
pub mod vfs;
pub use executor::block_on;
pub use ipc::{Channel, KernelTransport, Message, Rpc, Socket, SocketInfo};
pub use memory::Memory;
pub use startup::{
    HardwareResourceKind, NamespaceEntry, ServiceGrant, Startup, StartupHardwareResource,
    TraceProducerDescriptor,
};
pub use syscall::{exit, log, yield_now};

#[macro_export]
macro_rules! entry {
    ($entry:path) => {
        #[global_allocator]
        static HEAP: bexos_userspace::Heap = bexos_userspace::Heap::new();
        #[unsafe(no_mangle)]
        pub extern "C" fn _start(channel: u64) -> ! {
            unsafe {
                if let Some((base, size)) = bexos_userspace::initial_heap_region() {
                    HEAP.init(base, size);
                }
            }
            $entry(channel)
        }
        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
            bexos_userspace::panic_log(info);
            bexos_userspace::exit()
        }
    };
}
pub use bexos_allocator::Heap;
pub use bexos_boot::USER_HEAP_CHUNK_SIZE;
pub fn initial_heap_region() -> Option<(usize, usize)> {
    use kernel_fidl::{FidlDecode, FidlEncode};

    let heap = syscall::heap_vmar();
    if heap == 0 {
        return None;
    }
    let size = USER_HEAP_CHUNK_SIZE;
    let mut request = [0; 128];
    let mut request_handles = [kernel_fidl::HandleRef { raw: 0 }; 4];
    let encoded = kernel_fidl::VirtualMemoryCreateVmoRequest {
        size_bytes: size,
        flags: kernel_fidl::VmoFlags(0),
    }
    .encode(&mut request, &mut request_handles)
    .ok()?;
    let mut response = [0; 128];
    let mut out_handles = [0; 4];
    let create_ordinal = kernel_fidl::VIRTUAL_MEMORY_PUBLIC_METHODS
        .iter()
        .find(|method| method.name == "CreateVmo")?
        .ordinal;
    let (response_bytes, response_handles) = syscall::fidl(
        2,
        create_ordinal,
        &request[..encoded.bytes],
        &[],
        &mut response,
        &mut out_handles,
    )
    .ok()?;
    let refs = [kernel_fidl::HandleRef {
        raw: out_handles[0],
    }];
    let created = kernel_fidl::VirtualMemoryCreateVmoResponse::decode(
        &response[..response_bytes],
        &refs[..response_handles],
    )
    .ok()?;
    if created.status != kernel_fidl::Status::Ok {
        return None;
    }
    let vmo = created.vmo.raw;

    let encoded = kernel_fidl::VirtualMemoryMapVmoRequest {
        vmar: kernel_fidl::HandleRef { raw: heap },
        vmo: kernel_fidl::HandleRef { raw: vmo },
        vmo_offset: 0,
        vmar_offset: 0,
        size_bytes: size,
        flags: kernel_fidl::VmarFlags(0x1 | 0x2),
    }
    .encode(&mut request, &mut request_handles)
    .ok()?;
    let map_ordinal = kernel_fidl::VIRTUAL_MEMORY_PUBLIC_METHODS
        .iter()
        .find(|method| method.name == "MapVmo")?
        .ordinal;
    let (response_bytes, response_handles) = syscall::fidl(
        2,
        map_ordinal,
        &request[..encoded.bytes],
        &[heap, vmo],
        &mut response,
        &mut out_handles,
    )
    .ok()?;
    let mapped = kernel_fidl::VirtualMemoryMapVmoResponse::decode(
        &response[..response_bytes],
        &refs[..response_handles],
    )
    .ok()?;
    if mapped.status != kernel_fidl::Status::Ok {
        return None;
    }
    let _ = close_raw(vmo);
    let base = mapped.mapped_vaddr;
    Some((base as usize, size as usize))
}

fn close_raw(handle: u64) -> Option<()> {
    use kernel_fidl::{FidlDecode, FidlEncode};

    let mut request = [0; 64];
    let mut request_handles = [kernel_fidl::HandleRef { raw: 0 }; 1];
    let encoded = kernel_fidl::ObjectControlCloseRequest {
        object: kernel_fidl::HandleRef { raw: handle },
    }
    .encode(&mut request, &mut request_handles)
    .ok()?;
    let ordinal = kernel_fidl::OBJECT_CONTROL_PUBLIC_METHODS
        .iter()
        .find(|method| method.name == "Close")?
        .ordinal;
    let mut response = [0; 64];
    let mut out_handles = [0; 1];
    let (response_bytes, response_handles) = syscall::fidl(
        5,
        ordinal,
        &request[..encoded.bytes],
        &[handle],
        &mut response,
        &mut out_handles,
    )
    .ok()?;
    let decoded = kernel_fidl::ObjectControlCloseResponse::decode(
        &response[..response_bytes],
        &[][..response_handles],
    )
    .ok()?;
    (decoded.status == kernel_fidl::Status::Ok).then_some(())
}
pub fn panic_log(info: &core::panic::PanicInfo<'_>) {
    use core::fmt::Write;
    struct Log;
    impl Write for Log {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            log(s);
            Ok(())
        }
    }
    let _ = writeln!(Log, "guest panic: {info}");
}
pub mod live_migration;
pub mod migration;

pub mod service_binding;
pub mod service_control;
pub mod service_directory;

pub mod checkpoint;
pub mod clock;
pub mod config;
pub mod random;

#[cfg(test)]
mod startup_compat_tests;
