use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::{BufferMode, CATEGORY_APP_CUSTOM, TraceConfig, TraceEvent, TraceEventKind};

pub const MAX_PRODUCER_BUFFER_SIZE: usize = 2 * 1024 * 1024;
pub const HEADER_SIZE: usize = 4096;
pub const SLOT_SIZE: usize = 160;
const HEADER_MAGIC: u64 = 0x4258_5452_4143_4531;
const HEADER_VERSION: u32 = 1;
const SLOT_EMPTY: u64 = 0;
const SLOT_RESERVED: u64 = 1;

#[repr(C, align(8))]
pub struct SharedTraceHeader {
    magic: AtomicU64,
    version: AtomicU32,
    header_size: AtomicU32,
    session_epoch: AtomicU64,
    enabled_categories: AtomicU32,
    buffer_mode: AtomicU32,
    active_len: AtomicUsize,
    slot_count: AtomicUsize,
    write_sequence: AtomicU64,
    dropped_count: AtomicU64,
}

#[repr(C, align(8))]
pub struct SharedTraceSlot {
    commit_sequence: AtomicU64,
    timestamp_ns: AtomicU64,
    pid: AtomicU64,
    tid: AtomicU64,
    category: AtomicU32,
    kind: AtomicU32,
    flow_id: AtomicU64,
    value: AtomicU64,
    arg0_key_len: AtomicU32,
    arg1_key_len: AtomicU32,
    arg0_value: AtomicU64,
    arg1_value: AtomicU64,
    name_len: AtomicU32,
    _reserved: AtomicU32,
    name: [AtomicU8; 48],
    arg0_key: [AtomicU8; 8],
    arg1_key: [AtomicU8; 8],
    _pad: [u8; 8],
}

const _: () = assert!(core::mem::size_of::<SharedTraceHeader>() <= HEADER_SIZE);
const _: () = assert!(core::mem::size_of::<SharedTraceSlot>() == SLOT_SIZE);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedTraceWriter {
    header: *mut SharedTraceHeader,
    slots: *mut SharedTraceSlot,
    len: usize,
    producer_id: u64,
    pid: u64,
    tid: u64,
}

unsafe impl Send for SharedTraceWriter {}
unsafe impl Sync for SharedTraceWriter {}

impl SharedTraceWriter {
    pub unsafe fn from_raw_parts(
        ptr: *mut u8,
        len: usize,
        producer_id: u64,
        pid: u64,
        tid: u64,
    ) -> Option<Self> {
        if ptr.is_null() || len < HEADER_SIZE + SLOT_SIZE {
            return None;
        }
        let header = ptr.cast::<SharedTraceHeader>();
        let slots = unsafe { ptr.add(HEADER_SIZE).cast::<SharedTraceSlot>() };
        Some(Self {
            header,
            slots,
            len,
            producer_id,
            pid,
            tid,
        })
    }

    pub fn configure(&self, config: &TraceConfig, session_epoch: u64) {
        let active_len = active_len(self.len, config.buffer_size_kb);
        let slot_count = (active_len.saturating_sub(HEADER_SIZE) / SLOT_SIZE).max(1);
        let header = unsafe { &*self.header };
        header.magic.store(HEADER_MAGIC, Ordering::Release);
        header.version.store(HEADER_VERSION, Ordering::Release);
        header
            .header_size
            .store(HEADER_SIZE as u32, Ordering::Release);
        header.session_epoch.store(session_epoch, Ordering::Release);
        header
            .enabled_categories
            .store(config.categories, Ordering::Release);
        header
            .buffer_mode
            .store(config.buffer_mode.to_wire(), Ordering::Release);
        header.active_len.store(active_len, Ordering::Release);
        header.slot_count.store(slot_count, Ordering::Release);
        header.write_sequence.store(0, Ordering::Release);
        header.dropped_count.store(0, Ordering::Release);
        for index in 0..slot_count {
            unsafe {
                (*self.slots.add(index))
                    .commit_sequence
                    .store(SLOT_EMPTY, Ordering::Release);
            }
        }
    }

    pub fn disable(&self) {
        let header = unsafe { &*self.header };
        header.enabled_categories.store(0, Ordering::Release);
    }

    pub fn record(
        &self,
        kind: TraceEventKind,
        category: u32,
        name: &str,
        flow_id: u64,
        value: i64,
    ) {
        let header = unsafe { &*self.header };
        let enabled = header.enabled_categories.load(Ordering::Acquire);
        if enabled & category == 0 {
            return;
        }
        let slot_count = header.slot_count.load(Ordering::Acquire);
        if slot_count == 0 {
            header.dropped_count.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let sequence = reserve_sequence(header, slot_count);
        let Some(sequence) = sequence else {
            header.dropped_count.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let index = (sequence as usize - 1) % slot_count;
        let slot = unsafe { &*self.slots.add(index) };
        slot.commit_sequence.store(SLOT_RESERVED, Ordering::Release);
        let event = SlotEvent {
            timestamp_ns: synthetic_timestamp(),
            pid: self.pid.max(self.producer_id),
            tid: self.tid,
            category,
            kind,
            name,
            flow_id,
            value,
        };
        write_slot(slot, event);
        slot.commit_sequence.store(sequence + 1, Ordering::Release);
    }

    pub fn dropped_count(&self) -> u64 {
        unsafe { &*self.header }
            .dropped_count
            .load(Ordering::Acquire)
    }
}

pub struct SharedTraceReader {
    header: *const SharedTraceHeader,
    slots: *const SharedTraceSlot,
}

impl SharedTraceReader {
    pub unsafe fn from_raw_parts(ptr: *const u8, len: usize) -> Option<Self> {
        if ptr.is_null() || len < HEADER_SIZE + SLOT_SIZE {
            return None;
        }
        Some(Self {
            header: ptr.cast::<SharedTraceHeader>(),
            slots: unsafe { ptr.add(HEADER_SIZE).cast::<SharedTraceSlot>() },
        })
    }

    pub fn events(&self) -> Vec<TraceEvent> {
        let mut cursor = 0;
        self.drain_since(&mut cursor)
    }

    pub fn drain_since(&self, cursor: &mut u64) -> Vec<TraceEvent> {
        let header = unsafe { &*self.header };
        if header.magic.load(Ordering::Acquire) != HEADER_MAGIC {
            return Vec::new();
        }
        let slot_count = header.slot_count.load(Ordering::Acquire);
        if slot_count == 0 {
            return Vec::new();
        }
        let written = header.write_sequence.load(Ordering::Acquire) as usize;
        let readable = slot_count.min(written);
        let retained_start = written.saturating_sub(readable);
        let start = retained_start.max(*cursor as usize);
        let mut out = Vec::new();
        for sequence in start..written {
            let index = sequence % slot_count;
            let slot = unsafe { &*self.slots.add(index) };
            if let Some(event) = read_slot(slot, (sequence as u64) + 2) {
                out.push(event);
            }
        }
        *cursor = written as u64;
        out
    }

    pub fn dropped_count(&self) -> u64 {
        unsafe { &*self.header }
            .dropped_count
            .load(Ordering::Acquire)
    }
}

struct SlotEvent<'a> {
    timestamp_ns: u64,
    pid: u64,
    tid: u64,
    category: u32,
    kind: TraceEventKind,
    name: &'a str,
    flow_id: u64,
    value: i64,
}

fn write_slot(slot: &SharedTraceSlot, event: SlotEvent<'_>) {
    slot.timestamp_ns
        .store(event.timestamp_ns, Ordering::Relaxed);
    slot.pid.store(event.pid, Ordering::Relaxed);
    slot.tid.store(event.tid, Ordering::Relaxed);
    slot.category.store(event.category, Ordering::Relaxed);
    slot.kind.store(event.kind.code() as u32, Ordering::Relaxed);
    slot.flow_id.store(event.flow_id, Ordering::Relaxed);
    slot.value.store(event.value as u64, Ordering::Relaxed);
    write_bytes(&slot.name, &slot.name_len, event.name);
    slot.arg0_key_len.store(0, Ordering::Relaxed);
    slot.arg1_key_len.store(0, Ordering::Relaxed);
    slot.arg0_value.store(0, Ordering::Relaxed);
    slot.arg1_value.store(0, Ordering::Relaxed);
}

fn read_slot(slot: &SharedTraceSlot, expected_commit: u64) -> Option<TraceEvent> {
    if slot.commit_sequence.load(Ordering::Acquire) != expected_commit {
        return None;
    }
    let kind = TraceEventKind::from_code(slot.kind.load(Ordering::Acquire) as u8)?;
    let name = read_string(&slot.name, slot.name_len.load(Ordering::Acquire) as usize)?;
    let event = TraceEvent::new(
        slot.timestamp_ns.load(Ordering::Acquire),
        slot.pid.load(Ordering::Acquire),
        slot.tid.load(Ordering::Acquire),
        slot.category.load(Ordering::Acquire),
        kind,
        &name,
    )
    .flow(slot.flow_id.load(Ordering::Acquire))
    .counter(slot.value.load(Ordering::Acquire) as i64);
    (slot.commit_sequence.load(Ordering::Acquire) == expected_commit).then_some(event)
}

fn write_bytes<const N: usize>(dst: &[AtomicU8; N], len: &AtomicU32, value: &str) {
    let bytes = value.as_bytes();
    let count = bytes.len().min(N);
    // Readers can overlap a slot overwrite. Atomic bytes avoid both immutable
    // reference mutation and data races; commit_sequence validates the record.
    for (index, byte) in dst.iter().enumerate() {
        byte.store(bytes.get(index).copied().unwrap_or(0), Ordering::Relaxed);
    }
    len.store(count as u32, Ordering::Relaxed);
}

fn read_string<const N: usize>(src: &[AtomicU8; N], len: usize) -> Option<String> {
    if len > N {
        return None;
    }
    let bytes: Vec<u8> = src[..len]
        .iter()
        .map(|b| b.load(Ordering::Relaxed))
        .collect();
    core::str::from_utf8(&bytes).ok().map(ToString::to_string)
}

fn active_len(len: usize, requested_kb: u32) -> usize {
    let requested = (requested_kb as usize).saturating_mul(1024);
    requested.clamp(HEADER_SIZE + SLOT_SIZE, len.min(MAX_PRODUCER_BUFFER_SIZE))
}

fn reserve_sequence(header: &SharedTraceHeader, slot_count: usize) -> Option<u64> {
    if header.buffer_mode.load(Ordering::Acquire) != BufferMode::OneshotStopOnFull.to_wire() {
        return Some(header.write_sequence.fetch_add(1, Ordering::AcqRel) + 1);
    }
    loop {
        let current = header.write_sequence.load(Ordering::Acquire);
        if current as usize >= slot_count {
            return None;
        }
        if header
            .write_sequence
            .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Some(current + 1);
        }
    }
}

static GLOBAL_PTR: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());
static GLOBAL_LEN: AtomicUsize = AtomicUsize::new(0);
static GLOBAL_PRODUCER: AtomicU64 = AtomicU64::new(0);
static GLOBAL_PID: AtomicU64 = AtomicU64::new(0);
static GLOBAL_TID: AtomicU64 = AtomicU64::new(0);
static CLOCK: AtomicU64 = AtomicU64::new(1);

pub unsafe fn init_global_writer(
    ptr: *mut u8,
    len: usize,
    producer_id: u64,
    pid: u64,
    tid: u64,
) -> Option<SharedTraceWriter> {
    let writer = unsafe { SharedTraceWriter::from_raw_parts(ptr, len, producer_id, pid, tid)? };
    GLOBAL_PTR.store(ptr, Ordering::Release);
    GLOBAL_LEN.store(len, Ordering::Release);
    GLOBAL_PRODUCER.store(producer_id, Ordering::Release);
    GLOBAL_PID.store(pid, Ordering::Release);
    GLOBAL_TID.store(tid, Ordering::Release);
    Some(writer)
}

fn global_writer() -> Option<SharedTraceWriter> {
    let ptr = GLOBAL_PTR.load(Ordering::Acquire);
    if ptr.is_null() {
        return None;
    }
    Some(SharedTraceWriter {
        header: ptr.cast::<SharedTraceHeader>(),
        slots: unsafe { ptr.add(HEADER_SIZE).cast::<SharedTraceSlot>() },
        len: GLOBAL_LEN.load(Ordering::Acquire),
        producer_id: GLOBAL_PRODUCER.load(Ordering::Acquire),
        pid: GLOBAL_PID.load(Ordering::Acquire),
        tid: GLOBAL_TID.load(Ordering::Acquire),
    })
}

pub fn record_instant(category: u32, name: &'static str) {
    if let Some(writer) = global_writer() {
        writer.record(TraceEventKind::Instant, category, name, 0, 0);
    }
}

pub fn record_counter(category: u32, name: &'static str, value: i64) {
    if let Some(writer) = global_writer() {
        writer.record(TraceEventKind::Counter, category, name, 0, value);
    }
}

pub fn record_flow_begin(category: u32, name: &'static str, flow_id: u64) {
    if let Some(writer) = global_writer() {
        writer.record(TraceEventKind::FlowBegin, category, name, flow_id, 0);
    }
}

pub fn record_flow_step(category: u32, name: &'static str, flow_id: u64) {
    if let Some(writer) = global_writer() {
        writer.record(TraceEventKind::FlowStep, category, name, flow_id, 0);
    }
}

pub fn record_flow_end(category: u32, name: &'static str, flow_id: u64) {
    if let Some(writer) = global_writer() {
        writer.record(TraceEventKind::FlowEnd, category, name, flow_id, 0);
    }
}

pub fn record_slice_begin(category: u32, name: &'static str) -> TraceScopeGuard {
    if let Some(writer) = global_writer() {
        writer.record(TraceEventKind::SliceBegin, category, name, 0, 0);
        TraceScopeGuard {
            category,
            name,
            active: true,
        }
    } else {
        TraceScopeGuard::disabled()
    }
}

pub fn record_slice_end(category: u32, name: &'static str) {
    if let Some(writer) = global_writer() {
        writer.record(TraceEventKind::SliceEnd, category, name, 0, 0);
    }
}

pub struct TraceScopeGuard {
    category: u32,
    name: &'static str,
    active: bool,
}

impl TraceScopeGuard {
    pub const fn disabled() -> Self {
        Self {
            category: CATEGORY_APP_CUSTOM,
            name: "",
            active: false,
        }
    }
}

impl Drop for TraceScopeGuard {
    fn drop(&mut self) {
        if self.active {
            record_slice_end(self.category, self.name);
        }
    }
}

fn synthetic_timestamp() -> u64 {
    CLOCK.fetch_add(1, Ordering::Relaxed)
}
