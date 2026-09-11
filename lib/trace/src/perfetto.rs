use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::{LEGACY_BEXOS_FXT_MAGIC, TraceEvent, TraceEventKind, category_name};

pub fn looks_like_perfetto_trace(bytes: &[u8]) -> bool {
    !bytes.is_empty() && !bytes.starts_with(LEGACY_BEXOS_FXT_MAGIC)
}

pub fn export_perfetto_trace(events: &[TraceEvent]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut sorted = events.to_vec();
    sorted.sort_by_key(|event| (event.timestamp_ns, event.pid, event.tid, event.kind.code()));
    let mut described_tracks = BTreeMap::<u64, ()>::new();
    for event in &sorted {
        let uuid = track_uuid(event.pid, event.tid);
        if described_tracks.insert(uuid, ()).is_none() {
            let mut desc = Vec::new();
            put_varint_field(&mut desc, 1, uuid);
            if event.tid == 0 {
                put_string_field(&mut desc, 2, "process");
                let mut proc = Vec::new();
                put_varint_field(&mut proc, 1, event.pid);
                put_string_field(&mut proc, 6, "bexos-process");
                put_len_field(&mut desc, 5, &proc);
            } else {
                put_string_field(&mut desc, 2, "thread");
                put_varint_field(&mut desc, 5, track_uuid(event.pid, 0));
                let mut thread = Vec::new();
                put_varint_field(&mut thread, 1, event.pid);
                put_varint_field(&mut thread, 2, event.tid);
                put_string_field(&mut thread, 5, "bexos-thread");
                put_len_field(&mut desc, 6, &thread);
            }
            let mut packet = Vec::new();
            put_varint_field(&mut packet, 8, event.timestamp_ns);
            put_len_field(&mut packet, 60, &desc);
            put_len_field(&mut out, 1, &packet);
        }
    }
    for event in &sorted {
        let mut packet = Vec::new();
        put_varint_field(&mut packet, 8, event.timestamp_ns);
        let mut track = Vec::new();
        put_varint_field(&mut track, 11, track_uuid(event.pid, event.tid));
        put_string_field(&mut track, 22, category_name(event.category));
        put_string_field(&mut track, 23, &event.name);
        match event.kind {
            TraceEventKind::SliceBegin => put_varint_field(&mut track, 9, 1),
            TraceEventKind::SliceEnd => put_varint_field(&mut track, 9, 2),
            TraceEventKind::Instant | TraceEventKind::Metadata => {
                put_varint_field(&mut track, 9, 3)
            }
            TraceEventKind::Counter | TraceEventKind::DroppedEvents => {
                put_varint_field(&mut track, 9, 4);
                put_svarint_field(&mut track, 30, event.value);
            }
            TraceEventKind::FlowBegin => {
                put_varint_field(&mut track, 9, 1);
                put_varint_field(&mut track, 47, event.flow_id);
            }
            TraceEventKind::FlowStep => {
                put_varint_field(&mut track, 9, 3);
                put_varint_field(&mut track, 48, event.flow_id);
            }
            TraceEventKind::FlowEnd => {
                put_varint_field(&mut track, 9, 2);
                put_varint_field(&mut track, 48, event.flow_id);
            }
        }
        for annotation in event.annotations.iter().flatten() {
            let mut debug = Vec::new();
            put_string_field(&mut debug, 10, &annotation.key);
            put_svarint_field(&mut debug, 2, annotation.value);
            put_len_field(&mut track, 4, &debug);
        }
        put_len_field(&mut packet, 11, &track);
        put_len_field(&mut out, 1, &packet);
    }
    out
}

const fn track_uuid(pid: u64, tid: u64) -> u64 {
    0xBEE0_0000_0000u64
        .wrapping_add(pid.wrapping_shl(16))
        .wrapping_add(tid)
}

fn put_string_field(out: &mut Vec<u8>, field: u64, value: &str) {
    put_len_field(out, field, value.as_bytes());
}

fn put_len_field(out: &mut Vec<u8>, field: u64, value: &[u8]) {
    put_key(out, field, 2);
    put_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn put_varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    put_key(out, field, 0);
    put_varint(out, value);
}

fn put_svarint_field(out: &mut Vec<u8>, field: u64, value: i64) {
    put_varint_field(out, field, ((value << 1) ^ (value >> 63)) as u64);
}

fn put_key(out: &mut Vec<u8>, field: u64, wire: u8) {
    put_varint(out, (field << 3) | u64::from(wire));
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}
