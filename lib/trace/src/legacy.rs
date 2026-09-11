use alloc::vec::Vec;

use crate::{TraceEvent, category_name};

pub const LEGACY_BEXOS_FXT_MAGIC: &[u8; 10] = b"FXT\0BEXOS\0";

pub fn export_legacy_bexos_fxt(events: &[TraceEvent]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(LEGACY_BEXOS_FXT_MAGIC);
    put_u64(&mut out, events.len() as u64);
    for event in events {
        put_u8(&mut out, event.kind.code());
        put_u32(&mut out, event.category);
        put_u64(&mut out, event.timestamp_ns);
        put_u64(&mut out, event.pid);
        put_u64(&mut out, event.tid);
        put_u64(&mut out, event.flow_id);
        put_i64(&mut out, event.value);
        put_str(&mut out, category_name(event.category));
        put_str(&mut out, &event.name);
        put_u8(
            &mut out,
            event.annotations.iter().filter(|a| a.is_some()).count() as u8,
        );
        for annotation in event.annotations.iter().flatten() {
            put_str(&mut out, &annotation.key);
            put_i64(&mut out, annotation.value);
        }
    }
    out
}

pub fn looks_like_legacy_bexos_fxt(bytes: &[u8]) -> bool {
    bytes.starts_with(LEGACY_BEXOS_FXT_MAGIC)
}

fn put_u8(out: &mut Vec<u8>, value: u8) {
    out.push(value);
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, value: &str) {
    put_u32(out, value.len() as u32);
    out.extend_from_slice(value.as_bytes());
}
