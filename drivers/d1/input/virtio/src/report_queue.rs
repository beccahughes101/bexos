//! Validate a complete device-owned used batch before returning descriptors.
use bexos_flatland_input::virtio::RawEvent;
use core::sync::atomic::{Ordering, fence};
#[derive(Debug, PartialEq)]
pub enum Error {
    Bounds,
    Count,
    Descriptor,
}
pub struct Batch {
    pub count: usize,
    pub ids: [u16; 64],
}
pub fn read(
    queue: &[u8],
    reports: &[u8],
    used: u16,
    out: &mut [RawEvent; 64],
) -> Result<Batch, Error> {
    if queue.len() < 8196 + 64 * 8 || queue.as_ptr() as usize % 4 != 0 || reports.len() < 512 {
        return Err(Error::Bounds);
    }
    let produced =
        u16::from_le(unsafe { core::ptr::read_volatile(queue.as_ptr().add(8194).cast::<u16>()) });
    let count = produced.wrapping_sub(used) as usize;
    if count > 64 {
        return Err(Error::Count);
    }
    fence(Ordering::SeqCst);
    let mut batch = Batch {
        count,
        ids: [0; 64],
    };
    let mut seen = [false; 64];
    for index in 0..count {
        let offset = 8196 + (used.wrapping_add(index as u16) as usize % 64) * 8;
        let id = u32::from_le(unsafe {
            core::ptr::read_volatile(queue.as_ptr().add(offset).cast::<u32>())
        }) as usize;
        let length = u32::from_le(unsafe {
            core::ptr::read_volatile(queue.as_ptr().add(offset + 4).cast::<u32>())
        });
        if id >= 64 || length != 8 || seen[id] {
            return Err(Error::Descriptor);
        }
        seen[id] = true;
        batch.ids[index] = id as u16;
    }
    for (event, id) in out.iter_mut().zip(batch.ids).take(count) {
        let bytes = unsafe {
            core::ptr::read_volatile(reports.as_ptr().add(id as usize * 8).cast::<[u8; 8]>())
        };
        *event = RawEvent::decode(bytes);
    }
    Ok(batch)
}
