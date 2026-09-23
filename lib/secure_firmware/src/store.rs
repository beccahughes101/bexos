//! Fixed, bounded A/B image slots on the monitor-owned firmware disk. Disk
//! contents are untrusted; only signature verification and a protected journal
//! identity authorize execution. No disk header can advance a generation.
use crate::selection::{Identity, Slot, component_number};
use crate::{Architecture, Component, MAX_BUNDLE_BYTES, Verified, verify};

pub const SECTOR_BYTES: usize = 512;
pub const SLOT_BYTES: u64 = 65 * 1024 * 1024;
// QEMU's file-backed RAM rounds its mapping to the host page size. Keep the
// shared medium 16 KiB aligned so the same durable A/B layout works on macOS
// hosts with 16 KiB pages and on 4 KiB Linux hosts.
pub const DISK_BYTES: u64 = (4096 + 4 * SLOT_BYTES + 16 * 1024 - 1) & !(16 * 1024 - 1);
const MAGIC: &[u8; 8] = b"BEXFD001";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Device,
    Capacity,
    Header,
    Image,
}
/// The backend must complete each operation before returning, bound waits,
/// report failed flushes, and preserve queue ownership through monitor transfer.
pub trait BlockDevice {
    /// Execution-owner policy, never a value read from the untrusted disk.
    fn architecture(&self) -> Architecture;
    fn sectors(&self) -> u64;
    fn read(&mut self, sector: u64, output: &mut [u8; SECTOR_BYTES]) -> Result<(), Error>;
    fn write(&mut self, sector: u64, input: &[u8; SECTOR_BYTES]) -> Result<(), Error>;
    fn flush(&mut self) -> Result<(), Error>;
}
pub const fn slot_sector(component: Component, slot: Slot) -> u64 {
    let index = (component_number(component) as u64 - 1) * 2 + slot.number() as u64 - 1;
    (4096 + index * SLOT_BYTES) / SECTOR_BYTES as u64
}
fn capacity(device: &impl BlockDevice) -> Result<(), Error> {
    if device.sectors() < DISK_BYTES / SECTOR_BYTES as u64 {
        Err(Error::Capacity)
    } else {
        Ok(())
    }
}
/// Install only to the inactive slot. The caller must retain the original
/// authenticated journal state and serialize other firmware updates.
pub fn install(
    device: &mut impl BlockDevice,
    component: Component,
    active: Identity,
    bundle: &[u8],
    root: &[u8],
    floor: u64,
    scratch: &mut [u8],
) -> Result<Identity, Error> {
    if scratch.len() < bundle.len() {
        return Err(Error::Capacity);
    }
    let candidate = write_inactive(device, component, active, bundle, root, floor)?;
    load(device, component, candidate, root, floor, scratch)?;
    Ok(candidate)
}
/// Reuse the root's upload allocation for reread authentication after all
/// writes are flushed. This bounds resident firmware memory to one maximum
/// bundle; the original immutable source is no longer needed at that point.
pub fn install_in_place(
    device: &mut impl BlockDevice,
    component: Component,
    active: Identity,
    bundle: &mut [u8],
    root: &[u8],
    floor: u64,
) -> Result<Identity, Error> {
    let candidate = write_inactive(device, component, active, bundle, root, floor)?;
    load(device, component, candidate, root, floor, bundle)?;
    Ok(candidate)
}
fn write_inactive(
    device: &mut impl BlockDevice,
    component: Component,
    active: Identity,
    bundle: &[u8],
    root: &[u8],
    floor: u64,
) -> Result<Identity, Error> {
    capacity(device)?;
    if !active.valid(true) {
        return Err(Error::Capacity);
    }
    let verified = verify(
        bundle,
        root,
        device.architecture(),
        component,
        active.generation,
        floor,
    )
    .map_err(|_| Error::Image)?;
    let candidate = Identity {
        slot: active.slot.other(),
        generation: verified.generation,
        digest: verified.digest,
        length: verified.image.len() as u64,
    };
    let first = slot_sector(component, candidate.slot);
    // Invalidate an older inactive image before changing any payload sectors.
    device.write(first, &[0; SECTOR_BYTES])?;
    device.flush()?;
    for (index, bytes) in bundle.chunks(SECTOR_BYTES).enumerate() {
        let mut sector = [0; SECTOR_BYTES];
        sector[..bytes.len()].copy_from_slice(bytes);
        device.write(first + 1 + index as u64, &sector)?;
    }
    device.flush()?;
    let mut header = [0; SECTOR_BYTES];
    header[..8].copy_from_slice(MAGIC);
    header[8..16].copy_from_slice(&(bundle.len() as u64).to_le_bytes());
    device.write(first, &header)?;
    device.flush()?;
    // This identity is private until the public installer rereads every byte.
    Ok(candidate)
}
pub fn load<'a>(
    device: &mut impl BlockDevice,
    component: Component,
    expected: Identity,
    root: &[u8],
    floor: u64,
    scratch: &'a mut [u8],
) -> Result<Verified<'a>, Error> {
    capacity(device)?;
    if !expected.valid(false) || expected.generation < floor {
        return Err(Error::Image);
    }
    let first = slot_sector(component, expected.slot);
    let mut header = [0; SECTOR_BYTES];
    device.read(first, &mut header)?;
    if &header[..8] != MAGIC || header[16..] != [0; SECTOR_BYTES - 16] {
        return Err(Error::Header);
    }
    let length = u64::from_le_bytes(header[8..16].try_into().unwrap());
    if !(crate::HEADER_BYTES as u64..=MAX_BUNDLE_BYTES as u64).contains(&length)
        || length > scratch.len() as u64
    {
        return Err(Error::Capacity);
    }
    let length = length as usize;
    for (index, bytes) in scratch[..length].chunks_mut(SECTOR_BYTES).enumerate() {
        let mut sector = [0; SECTOR_BYTES];
        device.read(first + 1 + index as u64, &mut sector)?;
        bytes.copy_from_slice(&sector[..bytes.len()]);
        if sector[bytes.len()..].iter().any(|b| *b != 0) {
            return Err(Error::Header);
        }
    }
    let verified = verify(
        &scratch[..length],
        root,
        device.architecture(),
        component,
        0,
        floor,
    )
    .map_err(|_| Error::Image)?;
    if verified.generation != expected.generation
        || verified.digest != expected.digest
        || verified.image.len() as u64 != expected.length
    {
        return Err(Error::Image);
    }
    Ok(verified)
}
