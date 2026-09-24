#![no_main]

bexos_libc::entry!(run);

fn run(channel: u64) -> ! {
    bexos_intel_nic_driver::run(channel, bexos_intel_nic::DeviceFamily::E1000e, "e1000e")
}
