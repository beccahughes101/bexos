//! Root-owned ATA PIO firmware medium. No guest DMA mapping, interrupt route,
//! descriptor ring, or userspace storage service is involved. Commands complete
//! synchronously; a timed-out transport is poisoned until explicitly reset.
use bexos_secure_firmware::store::{BlockDevice, DISK_BYTES, Error, SECTOR_BYTES};

const DATA: u16 = 0x1f0;
const STATUS: u16 = 0x1f7;
const CONTROL: u16 = 0x3f6;
const TIMEOUT_NS: u64 = 5_000_000_000;
pub trait Ports {
    fn read8(&mut self, port: u16) -> u8;
    fn write8(&mut self, port: u16, value: u8);
    fn read16(&mut self, port: u16) -> u16;
    fn write16(&mut self, port: u16, value: u16);
    fn now_ns(&self) -> u64;
}
/// May be reconstructed only at a completed command boundary. Its physical
/// controller is exclusively owned by the resident recovery nucleus.
pub struct Disk<P> {
    ports: P,
    sectors: u64,
    healthy: bool,
}
impl<P: Ports> Disk<P> {
    pub fn identify(mut ports: P) -> Result<Self, Error> {
        ports.write8(CONTROL, 2); // nIEN: root polls; no physical guest IRQ.
        let mut disk = Self {
            ports,
            sectors: 0,
            healthy: true,
        };
        disk.select(0);
        for port in 0x1f2..=0x1f5 {
            disk.ports.write8(port, 0);
        }
        disk.ports.write8(STATUS, 0xec);
        let started = disk.ports.now_ns();
        disk.wait(started, true)?;
        if disk.ports.read8(0x1f4) != 0 || disk.ports.read8(0x1f5) != 0 {
            return Err(Error::Device);
        }
        let mut words = [0; 256];
        for word in &mut words {
            *word = disk.ports.read16(DATA);
        }
        disk.wait(started, false)?;
        // LBA28 and FLUSH CACHE are mandatory; reject packet/removable media.
        if words[0] & 0x8080 != 0 || words[49] & (1 << 9) == 0 || words[83] & (1 << 12) == 0 {
            return Err(Error::Device);
        }
        disk.sectors = u64::from(words[60]) | u64::from(words[61]) << 16;
        if disk.sectors != DISK_BYTES / SECTOR_BYTES as u64 {
            return Err(Error::Capacity);
        }
        Ok(disk)
    }
    fn select(&mut self, sector: u64) {
        self.ports.write8(0x1f6, 0xe0 | ((sector >> 24) as u8 & 15));
        for _ in 0..4 {
            self.ports.read8(CONTROL);
        }
    }
    fn wait(&mut self, started: u64, data: bool) -> Result<(), Error> {
        loop {
            let status = self.ports.read8(STATUS);
            if status == 0 || status == 0xff {
                self.healthy = false;
                return Err(Error::Device);
            }
            if status & 0x80 == 0 {
                if status & 0x21 != 0 {
                    self.healthy = false;
                    return Err(Error::Device);
                }
                if (status & 8 != 0) == data {
                    return Ok(());
                }
            }
            let now = self.ports.now_ns();
            if now < started || now - started >= TIMEOUT_NS {
                self.healthy = false;
                return Err(Error::Device);
            }
        }
    }
    fn command(&mut self, sector: u64, op: u8) -> Result<u64, Error> {
        if !self.healthy {
            return Err(Error::Device);
        }
        if sector >= self.sectors || sector >= (1 << 28) {
            return Err(Error::Capacity);
        }
        let started = self.ports.now_ns();
        self.wait(started, false)?;
        self.select(sector);
        self.ports.write8(0x1f2, 1);
        self.ports.write8(0x1f3, sector as u8);
        self.ports.write8(0x1f4, (sector >> 8) as u8);
        self.ports.write8(0x1f5, (sector >> 16) as u8);
        self.ports.write8(STATUS, op);
        self.wait(started, true)?;
        Ok(started)
    }
}
impl<P: Ports> BlockDevice for Disk<P> {
    fn sectors(&self) -> u64 {
        self.sectors
    }
    fn read(&mut self, sector: u64, output: &mut [u8; SECTOR_BYTES]) -> Result<(), Error> {
        let started = self.command(sector, 0x20)?;
        for bytes in output.chunks_exact_mut(2) {
            bytes.copy_from_slice(&self.ports.read16(DATA).to_le_bytes());
        }
        self.wait(started, false)
    }
    fn write(&mut self, sector: u64, input: &[u8; SECTOR_BYTES]) -> Result<(), Error> {
        let started = self.command(sector, 0x30)?;
        for bytes in input.chunks_exact(2) {
            self.ports
                .write16(DATA, u16::from_le_bytes(bytes.try_into().unwrap()));
        }
        self.wait(started, false)
    }
    fn flush(&mut self) -> Result<(), Error> {
        if !self.healthy {
            return Err(Error::Device);
        }
        let started = self.ports.now_ns();
        self.wait(started, false)?;
        self.ports.write8(STATUS, 0xe7);
        self.wait(started, false)
    }
}

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
pub struct Native(());
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
impl Native {
    /// # Safety
    /// The caller exclusively owns the firmware controller at these ports,
    /// has initialized the resident HPET clock, and excludes both domains from
    /// these ports. Guest execution may resume between completed operations;
    /// controller ownership must remain resident throughout replacement.
    pub unsafe fn acquire() -> Self {
        Self(())
    }
}
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
impl Ports for Native {
    fn read8(&mut self, port: u16) -> u8 {
        let v;
        unsafe {
            core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack));
        }
        v
    }
    fn write8(&mut self, port: u16, value: u8) {
        unsafe {
            core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack));
        }
    }
    fn read16(&mut self, port: u16) -> u16 {
        let v;
        unsafe {
            core::arch::asm!("in ax, dx", in("dx") port, out("ax") v, options(nomem, nostack));
        }
        v
    }
    fn write16(&mut self, port: u16, value: u16) {
        unsafe {
            core::arch::asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack));
        }
    }
    fn now_ns(&self) -> u64 {
        unsafe { crate::clock::now_ns() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    extern crate std;
    #[derive(Default)]
    struct Faulted {
        time: Cell<u64>,
        writes: usize,
        data: usize,
        busy: bool,
    }
    impl Ports for Faulted {
        fn read8(&mut self, _: u16) -> u8 {
            self.time.set(self.time.get() + 1_000_000_000);
            if self.busy { 0x80 } else { 0x41 }
        }
        fn write8(&mut self, _: u16, _: u8) {
            self.writes += 1;
        }
        fn read16(&mut self, _: u16) -> u16 {
            self.data += 1;
            0
        }
        fn write16(&mut self, _: u16, _: u16) {
            self.data += 1;
        }
        fn now_ns(&self) -> u64 {
            self.time.get()
        }
    }
    #[test]
    fn errored_or_silent_controller_cannot_be_reused_without_reinitialization() {
        for busy in [false, true] {
            let mut disk = Disk {
                ports: Faulted {
                    busy,
                    ..Faulted::default()
                },
                sectors: DISK_BYTES / 512,
                healthy: true,
            };
            assert_eq!(disk.read(0, &mut [0; 512]), Err(Error::Device));
            assert!(!disk.healthy);
            assert_eq!(disk.write(0, &[0; 512]), Err(Error::Device));
            assert_eq!(disk.flush(), Err(Error::Device));
            assert_eq!((disk.ports.writes, disk.ports.data), (0, 0));
            assert!(disk.ports.time.get() <= TIMEOUT_NS);
        }
    }
    #[test]
    fn invalid_sector_never_reaches_the_controller() {
        let mut disk = Disk {
            ports: Faulted::default(),
            sectors: DISK_BYTES / 512,
            healthy: true,
        };
        for sector in [disk.sectors, 1 << 28, u64::MAX] {
            assert_eq!(disk.write(sector, &[0; 512]), Err(Error::Capacity));
        }
        assert!(disk.healthy);
        assert_eq!(
            (disk.ports.writes, disk.ports.data, disk.ports.time.get()),
            (0, 0, 0)
        );
    }
}
