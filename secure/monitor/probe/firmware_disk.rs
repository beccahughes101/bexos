//! Real controller read/write/flush and owner reconstruction, across two boots.
use bexos_secure_firmware::store::{BlockDevice, DISK_BYTES, SECTOR_BYTES};
use bexos_secure_monitor::firmware_disk::{Disk, Native};

pub unsafe fn verify() {
    assert!(unsafe { bexos_secure_monitor::clock::initialize() });
    let mut boot = [0];
    assert!(unsafe {
        bexos_secure_monitor::fw_cfg::read(b"opt/bexos/firmware-probe-boot", &mut boot)
    });
    assert!(matches!(boot[0], b'1' | b'2'));
    let mut disk =
        Disk::identify(unsafe { Native::acquire() }).expect("identify root firmware disk");
    assert_eq!(disk.sectors(), DISK_BYTES / SECTOR_BYTES as u64);
    let sector = disk.sectors() - 1;
    let mut bytes = [0; SECTOR_BYTES];
    disk.read(sector, &mut bytes).unwrap();
    assert_eq!(
        bytes,
        [if boot[0] == b'1' { 0 } else { 0x5a }; SECTOR_BYTES]
    );
    disk.write(sector, &[0x5a; SECTOR_BYTES]).unwrap();
    disk.flush().unwrap();
    assert!(disk.write(disk.sectors(), &[0; SECTOR_BYTES]).is_err());
    drop(disk);
    // There is no pending device command or DMA queue at this boundary.
    let mut restored = Disk::identify(unsafe { Native::acquire() }).unwrap();
    restored.read(sector, &mut bytes).unwrap();
    assert_eq!(bytes, [0x5a; SECTOR_BYTES]);
    crate::log("svm-probe: root firmware disk durable across owner reconstruction and reboot\n");
}
