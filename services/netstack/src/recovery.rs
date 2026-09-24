use alloc::vec::Vec;
use bexos_userspace::Memory;
use bexos_userspace::live_migration::State;
use net_fidl::Status;

use crate::migration::Runtime;

const MAGIC: &[u8; 8] = b"BEXNJNL1";
const VERSION: u32 = 1;
const HEADER_LEN: usize = 96;
const SLOT_LEN: usize = 32;

pub fn checkpoint(runtime: &Runtime, generation: u64) -> Result<(u64, u64), Status> {
    let mut payload = Vec::new();
    let keys = runtime.keys();
    push_u32(
        &mut payload,
        u32::try_from(keys.len()).map_err(|_| Status::ErrNoMemory)?,
    );
    for key in keys {
        let Some(record) = runtime
            .encode_record(key)
            .map_err(|_| Status::ErrInvalidArgs)?
        else {
            continue;
        };
        push_u64(&mut payload, key);
        push_u32(
            &mut payload,
            u32::try_from(record.len()).map_err(|_| Status::ErrNoMemory)?,
        );
        payload.extend_from_slice(&record);
    }
    if payload.is_empty() || payload.len() > 8 * 1024 * 1024 {
        return Err(Status::ErrNoMemory);
    }
    let journal_len = HEADER_LEN
        .checked_add(payload.len().checked_mul(2).ok_or(Status::ErrNoMemory)?)
        .ok_or(Status::ErrNoMemory)?;
    let mapped = journal_len.checked_add(4095).ok_or(Status::ErrNoMemory)? & !4095;
    let handle = Memory::create(mapped as u64, 0).map_err(map_kernel_status)?;
    let va = match Memory::map(handle, mapped as u64, 6) {
        Ok(va) => va,
        Err(error) => {
            let _ = Memory::close(handle);
            return Err(map_kernel_status(error));
        }
    };
    let bytes = unsafe { core::slice::from_raw_parts_mut(va as *mut u8, mapped) };
    bytes.fill(0);
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
    bytes[12..16].copy_from_slice(&architecture().to_le_bytes());
    bytes[16..24].copy_from_slice(&generation.to_le_bytes());
    let active = (generation & 1) as u32;
    bytes[24..28].copy_from_slice(&active.to_le_bytes());
    for slot in 0..2usize {
        let slot_base = 32 + slot * SLOT_LEN;
        let offset = HEADER_LEN + slot * payload.len();
        bytes[slot_base..slot_base + 8].copy_from_slice(&(offset as u64).to_le_bytes());
        bytes[slot_base + 8..slot_base + 16].copy_from_slice(&(payload.len() as u64).to_le_bytes());
        let slot_generation = if slot == active as usize {
            generation
        } else {
            generation.saturating_sub(1)
        };
        bytes[slot_base + 16..slot_base + 24].copy_from_slice(&slot_generation.to_le_bytes());
        bytes[slot_base + 24..slot_base + 28].copy_from_slice(&crc32(&payload).to_le_bytes());
        bytes[slot_base + 28..slot_base + 32].copy_from_slice(&1u32.to_le_bytes());
        bytes[offset..offset + payload.len()].copy_from_slice(&payload);
    }
    let _ = Memory::unmap(va, mapped as u64);
    Ok((handle, journal_len as u64))
}

pub fn adopt(
    runtime: &mut Runtime,
    handle: u64,
    len: u64,
    expected_generation: u64,
) -> Result<u64, Status> {
    let payload = read_payload(handle, len, expected_generation)?;
    let records = decode_records(&payload)?;

    // Service bindings are supplied by appd to the replacement instance and
    // must not be overwritten with process-local handles from the journal.
    let control = runtime.control;
    let migration = runtime.migration;
    let tls_trust = runtime.tls_trust;
    let clients = core::mem::take(&mut runtime.clients);
    let backend_clients = core::mem::take(&mut runtime.backend_clients);
    let controller_clients = core::mem::take(&mut runtime.controller_clients);
    for (key, record) in records {
        runtime
            .adopt_record(key, Some(&record))
            .map_err(|_| Status::ErrInvalidArgs)?;
    }
    runtime.control = control;
    runtime.migration = migration;
    runtime.tls_trust = tls_trust;
    runtime.clients = clients;
    runtime.backend_clients = backend_clients;
    runtime.controller_clients = controller_clients;
    runtime.generation = expected_generation;
    runtime.validate().map_err(|_| Status::ErrInvalidArgs)?;
    Ok(expected_generation)
}

fn read_payload(handle: u64, len: u64, expected_generation: u64) -> Result<Vec<u8>, Status> {
    if len < HEADER_LEN as u64 || len > 16 * 1024 * 1024 {
        return Err(Status::ErrInvalidArgs);
    }
    let mapped = len.checked_add(4095).ok_or(Status::ErrInvalidArgs)? & !4095;
    let va = Memory::map(handle, mapped, 2).map_err(|_| Status::ErrInvalidHandle)?;
    let result = unsafe {
        payload_bytes(
            core::slice::from_raw_parts(va as *const u8, len as usize),
            expected_generation,
        )
        .map(|payload| payload.to_vec())
    };
    let _ = Memory::unmap(va, mapped);
    result
}

#[cfg(test)]
fn validate_bytes(bytes: &[u8], expected_generation: u64) -> Result<u64, Status> {
    payload_bytes(bytes, expected_generation).map(|_| expected_generation)
}

fn payload_bytes<'a>(bytes: &'a [u8], expected_generation: u64) -> Result<&'a [u8], Status> {
    if bytes.get(..8) != Some(MAGIC.as_slice())
        || word32(bytes, 8)? != VERSION
        || word32(bytes, 12)? != architecture()
    {
        return Err(Status::ErrInvalidArgs);
    }
    let header_generation = word64(bytes, 16)?;
    let active = usize::try_from(word32(bytes, 24)?).map_err(|_| Status::ErrInvalidArgs)?;
    if active > 1 || header_generation != expected_generation {
        return Err(Status::ErrInvalidArgs);
    }
    let base = 32 + active * SLOT_LEN;
    let offset = usize::try_from(word64(bytes, base)?).map_err(|_| Status::ErrInvalidArgs)?;
    let length = usize::try_from(word64(bytes, base + 8)?).map_err(|_| Status::ErrInvalidArgs)?;
    let generation = word64(bytes, base + 16)?;
    let checksum = word32(bytes, base + 24)?;
    let committed = word32(bytes, base + 28)?;
    let end = offset.checked_add(length).ok_or(Status::ErrInvalidArgs)?;
    if committed != 1
        || generation != expected_generation
        || offset < HEADER_LEN
        || length == 0
        || end > bytes.len()
        || crc32(&bytes[offset..end]) != checksum
    {
        return Err(Status::ErrInvalidArgs);
    }
    Ok(&bytes[offset..end])
}

fn decode_records(bytes: &[u8]) -> Result<Vec<(u64, Vec<u8>)>, Status> {
    let mut offset = 0usize;
    let count = take_u32(bytes, &mut offset)? as usize;
    if count > 4096 {
        return Err(Status::ErrInvalidArgs);
    }
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let key = take_u64(bytes, &mut offset)?;
        let len = take_u32(bytes, &mut offset)? as usize;
        let end = offset.checked_add(len).ok_or(Status::ErrInvalidArgs)?;
        let record = bytes
            .get(offset..end)
            .ok_or(Status::ErrInvalidArgs)?
            .to_vec();
        offset = end;
        records.push((key, record));
    }
    if offset != bytes.len() || records.first().map(|record| record.0) != Some(0) {
        return Err(Status::ErrInvalidArgs);
    }
    Ok(records)
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn take_u32(bytes: &[u8], offset: &mut usize) -> Result<u32, Status> {
    let value = word32(bytes, *offset)?;
    *offset += 4;
    Ok(value)
}

fn take_u64(bytes: &[u8], offset: &mut usize) -> Result<u64, Status> {
    let value = word64(bytes, *offset)?;
    *offset += 8;
    Ok(value)
}

fn map_kernel_status(status: kernel_fidl::Status) -> Status {
    match status {
        kernel_fidl::Status::ErrNoMemory => Status::ErrNoMemory,
        kernel_fidl::Status::ErrAccessDenied => Status::ErrAccessDenied,
        _ => Status::ErrInvalidHandle,
    }
}

fn architecture() -> u32 {
    if cfg!(bexos_arch_x86_64) { 2 } else { 1 }
}

fn word32(bytes: &[u8], offset: usize) -> Result<u32, Status> {
    bytes
        .get(offset..offset + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(Status::ErrInvalidArgs)
}

fn word64(bytes: &[u8], offset: usize) -> Result<u64, Status> {
    bytes
        .get(offset..offset + 8)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(Status::ErrInvalidArgs)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_corruption_and_stale_generation() {
        let payload = b"checkpoint";
        let mut journal = vec![0; HEADER_LEN + payload.len()];
        journal[..8].copy_from_slice(MAGIC);
        journal[8..12].copy_from_slice(&VERSION.to_le_bytes());
        journal[12..16].copy_from_slice(&architecture().to_le_bytes());
        journal[16..24].copy_from_slice(&7u64.to_le_bytes());
        journal[24..28].copy_from_slice(&0u32.to_le_bytes());
        journal[32..40].copy_from_slice(&(HEADER_LEN as u64).to_le_bytes());
        journal[40..48].copy_from_slice(&(payload.len() as u64).to_le_bytes());
        journal[48..56].copy_from_slice(&7u64.to_le_bytes());
        journal[56..60].copy_from_slice(&crc32(payload).to_le_bytes());
        journal[60..64].copy_from_slice(&1u32.to_le_bytes());
        journal[HEADER_LEN..].copy_from_slice(payload);
        assert_eq!(validate_bytes(&journal, 7), Ok(7));
        assert_eq!(validate_bytes(&journal, 6), Err(Status::ErrInvalidArgs));
        *journal.last_mut().unwrap() ^= 1;
        assert_eq!(validate_bytes(&journal, 7), Err(Status::ErrInvalidArgs));
    }
}
