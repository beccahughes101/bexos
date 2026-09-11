#![no_main]
extern crate alloc;
use bexos_userspace::{Channel, Memory, Startup, config::ConfigTable, fs, log, yield_now};
bexos_libc::entry!(run);
mod progress;

const CONTENT: &[u8] = b"BexOS guest persistent data v1";

fn run(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).unwrap();
    bexos_libc::install_startup(&start);
    if start.namespace_paths != "/pkg;/data;/tmp" {
        log("storage-verify: unexpected namespace\n");
        bexos_userspace::exit();
    }
    let Some(pkg) = namespace_channel(&start, "/pkg") else {
        log("storage-verify: missing /pkg namespace\n");
        bexos_userspace::exit();
    };
    let Some(data) = namespace_channel(&start, "/data") else {
        log("storage-verify: missing /data namespace\n");
        bexos_userspace::exit();
    };
    let Some(tmp) = namespace_channel(&start, "/tmp") else {
        log("storage-verify: missing /tmp namespace\n");
        bexos_userspace::exit();
    };
    log(&alloc::format!(
        "storage-verify: startup arg0={} arg1={}
",
        start.arg0,
        start.arg1
    ));
    validate_config(&start);
    validate_tmp(tmp);
    assert_eq!(
        Memory::stats().unwrap().bootfs_pages,
        0,
        "BootFS must be gone before disk-only launch"
    );
    assert!(matches!(
        fs::open(pkg, "forbidden", 2 | 8),
        Err(fs_fidl::FsStatus::ReadOnly)
    ));
    if start.arg1 == 1 {
        log(
            "storage-verify: graphics boot fast path; durable write skipped
",
        );
        if let Err(e) = Startup::ready(control) {
            log(&alloc::format!(
                "storage-verify: ready failed {e:?}
"
            ));
        }
        loop {
            yield_now();
        }
    }
    validate_std_fs(start.arg0);
    if start.arg0 == 0xbeef {
        progress::run(control, pkg, data, Channel(start.resources[0]));
    }
    if start.arg0 > 1 {
        let file = match fs::open(data, "persistence.txt", 1) {
            Ok(file) => file,
            Err(e) => {
                log(&alloc::format!(
                    "storage-verify: persistent file missing on second boot {e:?}\n"
                ));
                bexos_userspace::exit();
            }
        };
        let bytes = match fs::read(file, 128) {
            Ok(bytes) => bytes,
            Err(e) => {
                log(&alloc::format!(
                    "storage-verify: persistent read failed {e:?}\n"
                ));
                bexos_userspace::exit();
            }
        };
        if bytes != CONTENT {
            log("storage-verify: persistent file contents differ\n");
            bexos_userspace::exit();
        }
        fs::close(file).unwrap();
        log("storage-verify: prior guest data survived reboot\n");
    }
    let file = fs::open(data, "persistence.txt", 1 | 2 | 8 | 16).unwrap();
    fs::write(file, CONTENT).unwrap();
    fs::sync_file(file).unwrap();
    fs::close(file).unwrap();
    log("storage-verify: disk-only ELF ran; package writes denied; data written\n");
    if let Err(e) = Startup::ready(control) {
        log(&alloc::format!("storage-verify: ready failed {e:?}\n"));
    }
    loop {
        yield_now();
    }
}

fn namespace_channel(start: &Startup, path: &str) -> Option<Channel> {
    start
        .namespace
        .iter()
        .find(|entry| entry.path == path)
        .map(|entry| Channel(entry.directory))
}

fn validate_std_fs(boot_count: u64) {
    const STD_CONTENT: &[u8] = b"BexOS std::fs persistent data v1";
    if boot_count > 1 {
        match std::fs::read("std_persistence.txt") {
            Ok(bytes) if bytes == STD_CONTENT => {}
            Ok(_) => {
                log("storage-verify: std::fs persistent file contents differ\n");
                bexos_userspace::exit();
            }
            Err(error) => {
                log(&alloc::format!(
                    "storage-verify: std::fs persistent read failed {error:?}\n"
                ));
                bexos_userspace::exit();
            }
        }
    }
    if let Err(error) = std::fs::write("std_persistence.txt", STD_CONTENT) {
        log(&alloc::format!(
            "storage-verify: std::fs persistent write failed {error:?}\n"
        ));
        bexos_userspace::exit();
    }
    match std::fs::metadata("std_persistence.txt") {
        Ok(metadata) if metadata.len() == STD_CONTENT.len() as u64 => {}
        Ok(_) => {
            log("storage-verify: std::fs metadata length differs\n");
            bexos_userspace::exit();
        }
        Err(error) => {
            log(&alloc::format!(
                "storage-verify: std::fs metadata failed {error:?}\n"
            ));
            bexos_userspace::exit();
        }
    }
}

fn validate_tmp(tmp: Channel) {
    if fs::open(tmp, "tmp_persistence.txt", 1).is_ok() {
        log("storage-verify: tmp file unexpectedly survived launch\n");
        bexos_userspace::exit();
    }
    let file = fs::open(tmp, "tmp_persistence.txt", 1 | 2 | 8 | 16).unwrap();
    fs::write(file, b"BexOS tmp data v1").unwrap();
    fs::close(file).unwrap();
    let file = fs::open(tmp, "tmp_persistence.txt", 1).unwrap();
    let bytes = fs::read(file, 128).unwrap();
    fs::close(file).unwrap();
    if bytes != b"BexOS tmp data v1" {
        log("storage-verify: tmp file contents differ\n");
        bexos_userspace::exit();
    }
}

fn validate_config(start: &Startup) {
    let Some(config) = start.config else {
        log("storage-verify: missing component config\n");
        bexos_userspace::exit();
    };
    if start.config_len == 0 {
        log("storage-verify: empty component config\n");
        bexos_userspace::exit();
    }
    let mapped_len = (start.config_len + 4095) & !4095;
    let va = Memory::map(config, mapped_len, 2).unwrap();
    let bytes = unsafe { core::slice::from_raw_parts(va as *const u8, start.config_len as usize) };
    let table = ConfigTable::parse(bytes).unwrap();
    if table.get_bool("enable_persistence") != Ok(true)
        || table.get_u32("retry_limit") != Ok(3)
        || table.get_string("channel") != Ok("qemu")
    {
        log("storage-verify: component config values differ\n");
        bexos_userspace::exit();
    }
    Memory::unmap(va, mapped_len).unwrap();
    Memory::close(config).unwrap();
}
