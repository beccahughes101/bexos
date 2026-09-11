//! Persistent e2e client: original open endpoints and positions span updates.
use super::*;
use alloc::vec::Vec;
use bexos_userspace::Rpc;
use block_fidl::*;
// The block protocol currently registers fixed 512 KiB buffers, matching the
// NVMe driver and filesystem clients.
const DMA_BUFFER_BYTES: u64 = 512 * 1024;

pub fn run(control: Channel, pkg: Channel, data: Channel, block: Channel) -> ! {
    let h = match Memory::create(4096, 0) {
        Ok(h) => h,
        Err(error) => {
            log(&alloc::format!(
                "storage-verify: progress memory create failed {error:?}\n"
            ));
            bexos_userspace::exit();
        }
    };
    let va = match Memory::map(h, 4096, 6) {
        Ok(va) => va,
        Err(error) => {
            log(&alloc::format!(
                "storage-verify: progress memory map failed {error:?}\n"
            ));
            bexos_userspace::exit();
        }
    };
    unsafe {
        core::ptr::write_volatile(va as *mut u64, 0);
        core::ptr::write_volatile((va + 8) as *mut u64, 0);
    }
    if let Err(error) = Startup::ready(control) {
        log(&alloc::format!(
            "storage-verify: progress ready failed {error:?}\n"
        ));
        bexos_userspace::exit();
    }
    let shared = match Memory::duplicate(h, 1 | 2 | 16 | 32) {
        Ok(shared) => shared,
        Err(error) => {
            log(&alloc::format!(
                "storage-verify: progress memory duplicate failed {error:?}\n"
            ));
            write_progress_error(va);
            bexos_userspace::exit();
        }
    };
    if let Err(error) = control.send(&[], &[shared]) {
        log(&alloc::format!(
            "storage-verify: progress handle send failed {error:?}\n"
        ));
        write_progress_error(va);
        bexos_userspace::exit();
    }
    let setup = match setup(pkg, data, block) {
        Ok(setup) => setup,
        Err(error) => {
            log(&alloc::format!(
                "storage-verify: progress setup failed {error:?}\n"
            ));
            write_progress_error(va);
            bexos_userspace::exit();
        }
    };
    let file = setup.file;
    let archive = setup.archive;
    let expected = setup.expected;
    let fifo = setup.fifo;
    let vmo_id = setup.vmo_id;
    let mut sequence = 0u64;
    let mut offset = 0usize;
    let mut completed = 0u64;
    loop {
        wait_for_progress_tick(control);
        sequence += 1;
        macro_rules! step {
            ($op:expr) => {{
                let value = $op?;
                completed += 1;
                unsafe {
                    core::ptr::write_volatile(va as *mut u64, completed);
                }
                value
            }};
        }
        let result = (|| {
            completed += 1;
            unsafe {
                core::ptr::write_volatile(va as *mut u64, completed);
            }
            if sequence % 16 != 0 {
                return Ok(());
            }
            step!(fs::seek(file, 0));
            step!(fs::write(file, &sequence.to_le_bytes()));
            step!(fs::seek(file, 0));
            if step!(fs::read(file, 3)) != sequence.to_le_bytes()[..3]
                || step!(fs::read(file, 5)) != sequence.to_le_bytes()[3..]
            {
                return Err(fs_fidl::FsStatus::Corrupt);
            }
            let len = 7.min(expected.len() - offset);
            if step!(fs::read(archive, len as u64)) != expected[offset..offset + len] {
                return Err(fs_fidl::FsStatus::Corrupt);
            }
            offset += len;
            if offset == expected.len() {
                step!(fs::seek(archive, 0));
                offset = 0;
            }
            if sequence % 64 == 0 {
                let request = BlockRequest {
                    req_id: sequence,
                    opcode: BlockOpcode::Read,
                    vmo_id,
                    vmo_offset_blocks: 0,
                    device_block_offset: 0,
                    block_count: 1,
                };
                let mut wire = [0u8; 64];
                let n = request
                    .encode(&mut wire, &mut [])
                    .map_err(|_| fs_fidl::FsStatus::Io)?
                    .bytes;
                Channel(fifo)
                    .send(&wire[..n], &[])
                    .map_err(|_| fs_fidl::FsStatus::Io)?;
                let response = Channel(fifo).recv().map_err(|_| fs_fidl::FsStatus::Io)?;
                let response = BlockResponse::decode(&response.bytes, &[])
                    .map_err(|_| fs_fidl::FsStatus::Io)?;
                if response.req_id != sequence || response.status != Status::Ok {
                    return Err(fs_fidl::FsStatus::Io);
                }
                completed += 1;
                unsafe {
                    core::ptr::write_volatile(va as *mut u64, completed);
                }
            }
            // Keep the original endpoints open. Node.Close synchronously flushes
            // the complete volume and belongs in the persistence workload, not
            // this client measuring request progress during bulk sync.
            Ok(())
        })();
        if result.is_err() {
            write_progress_error(va);
            bexos_userspace::exit();
        }
    }
}

struct Setup {
    file: Channel,
    archive: Channel,
    expected: Vec<u8>,
    fifo: u64,
    vmo_id: u32,
}

fn setup(pkg: Channel, data: Channel, block: Channel) -> Result<Setup, fs_fidl::FsStatus> {
    log("storage-verify: progress setup open data\n");
    let file = fs::open(data, "transplant-live.bin", 1 | 2 | 8 | 16)?;
    log("storage-verify: progress setup open manifest\n");
    let archive = fs::open(pkg, "package.bexmanifest", 1)?;
    log("storage-verify: progress setup read manifest\n");
    let expected = fs::read(archive, 32768)?;
    log("storage-verify: progress setup rewind manifest\n");
    fs::seek(archive, 0)?;
    log("storage-verify: progress setup get fifo\n");
    let fifo = call(block, 4, &BlockDeviceGetFifoRequest {})?;
    let fifo = BlockDeviceGetFifoResponse::decode(&fifo.0, &fifo.1)
        .map_err(|_| fs_fidl::FsStatus::Io)?
        .fifo_handle
        .raw;
    log("storage-verify: progress setup dma\n");
    let dma = Memory::create(DMA_BUFFER_BYTES, 0).map_err(|_| fs_fidl::FsStatus::Io)?;
    let shared = Memory::duplicate(dma, 1 | 2 | 4 | 16 | 32).map_err(|_| fs_fidl::FsStatus::Io)?;
    log("storage-verify: progress setup register buffer\n");
    let registered = call(
        block,
        2,
        &BlockDeviceRegisterBufferRequest {
            vmo: HandleRef { raw: shared },
        },
    )?;
    let vmo_id = BlockDeviceRegisterBufferResponse::decode(&registered.0, &registered.1)
        .map_err(|_| fs_fidl::FsStatus::Io)?
        .vmo_id;
    log("storage-verify: progress setup complete\n");
    Ok(Setup {
        file,
        archive,
        expected,
        fifo,
        vmo_id,
    })
}

fn write_progress_error(va: u64) {
    unsafe {
        core::ptr::write_volatile((va + 8) as *mut u64, 1);
    }
}

fn wait_for_progress_tick(control: Channel) {
    loop {
        match control.try_recv() {
            Ok(_) => {
                while control.try_recv().is_ok() {}
                return;
            }
            Err(_) => yield_now(),
        }
    }
}

fn call<Q: FidlEncode>(
    channel: Channel,
    ordinal: u64,
    q: &Q,
) -> Result<(Vec<u8>, Vec<HandleRef>), fs_fidl::FsStatus> {
    let mut bytes = [0u8; 128];
    let mut hs = [HandleRef { raw: 0 }; 4];
    let e = q
        .encode(&mut bytes, &mut hs)
        .map_err(|_| fs_fidl::FsStatus::Io)?;
    let m = Rpc(channel)
        .call_raw(
            ordinal,
            &bytes[..e.bytes],
            &hs[..e.handles]
                .iter()
                .map(|h| h.raw)
                .collect::<alloc::vec::Vec<_>>(),
            true,
        )
        .map_err(|_| fs_fidl::FsStatus::Io)?;
    Ok((
        m.bytes,
        m.handles
            .into_iter()
            .map(|h| HandleRef { raw: h })
            .collect(),
    ))
}
