#![no_main]
extern crate alloc;

use bexos_userspace::{Channel, Startup, fs, log, yield_now};

bexos_libc::entry!(run);

const MARKER: &[u8] = b"BexOS isolated user storage marker v1";

fn run(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).unwrap();
    bexos_libc::install_startup(&start);
    let Some(data) = start.namespace.iter().find(|entry| entry.path == "/data") else {
        log("user-storage-probe: missing /data namespace capability\n");
        bexos_userspace::exit();
    };
    let data = Channel(data.directory);
    if let Err(error) = Startup::ready(control) {
        log(&alloc::format!(
            "user-storage-probe: ready failed {error:?}\n"
        ));
        bexos_userspace::exit();
    }
    log("user-storage-probe: opening marker\n");
    let file = fs::open(data, "marker.bin", 1 | 2 | 8 | 16).unwrap();
    log("user-storage-probe: writing marker and growth extent\n");
    fs::write(file, MARKER).unwrap();
    fs::seek(file, 10 * 1024 * 1024).unwrap();
    fs::write(file, b"growth").unwrap();
    log("user-storage-probe: syncing user data\n");
    fs::sync_file(file).unwrap();
    log("user-storage-probe: user data synced\n");
    fs::close(file).unwrap();

    let file = fs::open(data, "marker.bin", 1).unwrap();
    let bytes = fs::read(file, MARKER.len() as u64).unwrap();
    fs::close(file).unwrap();
    if bytes != MARKER {
        log("user-storage-probe: marker mismatch\n");
        bexos_userspace::exit();
    }
    log("user-storage-probe: isolated user data written and reread\n");
    loop {
        yield_now();
    }
}
