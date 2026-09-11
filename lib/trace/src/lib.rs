#![no_std]

extern crate alloc;

mod categories;
mod event;
mod legacy;
mod perfetto;
mod session;
mod shared;

pub use categories::{
    CATEGORY_ALL, CATEGORY_APP_CUSTOM, CATEGORY_DEBUG_SERVICE, CATEGORY_IPC_MESSAGES,
    CATEGORY_KERNEL_SCHED, CATEGORY_NETWORK_STACK, CATEGORY_UI_FRAMES, CATEGORY_VFS_IO,
    category_list, category_name, parse_category_list,
};
pub use event::{TraceAnnotation, TraceEvent, TraceEventKind};
pub use legacy::{LEGACY_BEXOS_FXT_MAGIC, export_legacy_bexos_fxt, looks_like_legacy_bexos_fxt};
pub use perfetto::{export_perfetto_trace, looks_like_perfetto_trace};
pub use session::{
    BufferMode, TraceBuffer, TraceBufferSnapshot, TraceConfig, TraceOutputFormat, TraceProducer,
    TraceSession, TraceSessionSnapshot, TraceState,
};
pub use shared::{
    HEADER_SIZE, MAX_PRODUCER_BUFFER_SIZE, SLOT_SIZE, SharedTraceHeader, SharedTraceReader,
    SharedTraceSlot, SharedTraceWriter, TraceScopeGuard, init_global_writer, record_counter,
    record_flow_begin, record_flow_end, record_flow_step, record_instant, record_slice_begin,
    record_slice_end,
};

#[deprecated(note = "use export_legacy_bexos_fxt; this is not native Fuchsia FXT")]
pub fn export_fuchsia_trace(events: &[TraceEvent]) -> alloc::vec::Vec<u8> {
    export_legacy_bexos_fxt(events)
}

#[deprecated(note = "use looks_like_legacy_bexos_fxt")]
pub fn looks_like_fuchsia_trace(bytes: &[u8]) -> bool {
    looks_like_legacy_bexos_fxt(bytes)
}

pub struct ScopeGuard {
    _inner: TraceScopeGuard,
}

impl ScopeGuard {
    pub fn new(category: u32, name: &'static str) -> Self {
        Self {
            _inner: record_slice_begin(category, name),
        }
    }

    pub const fn disabled() -> Self {
        Self {
            _inner: TraceScopeGuard::disabled(),
        }
    }
}

#[macro_export]
macro_rules! trace_scope {
    ($category:expr, $name:expr $(, $key:expr => $value:expr)* $(,)?) => {
        let _bexos_trace_scope = $crate::ScopeGuard::new($category, $name);
        let _ = stringify!($($key => $value),*);
    };
    ($name:expr $(, $key:expr => $value:expr)* $(,)?) => {
        let _bexos_trace_scope = $crate::ScopeGuard::new($crate::CATEGORY_APP_CUSTOM, $name);
        let _ = stringify!($($key => $value),*);
    };
}

#[macro_export]
macro_rules! trace_instant {
    ($category:expr, $name:expr $(, $key:expr => $value:expr)* $(,)?) => {{
        $crate::record_instant($category, $name);
        let _ = stringify!($($key => $value),*);
    }};
    ($name:expr $(, $key:expr => $value:expr)* $(,)?) => {{
        $crate::record_instant($crate::CATEGORY_APP_CUSTOM, $name);
        let _ = stringify!($($key => $value),*);
    }};
}

#[macro_export]
macro_rules! trace_counter {
    ($category:expr, $name:expr, $value:expr $(,)?) => {{
        $crate::record_counter($category, $name, $value as i64);
    }};
    ($name:expr, $value:expr $(,)?) => {{
        $crate::record_counter($crate::CATEGORY_APP_CUSTOM, $name, $value as i64);
    }};
}

#[macro_export]
macro_rules! trace_flow_begin {
    ($category:expr, $name:expr, $flow_id:expr $(,)?) => {{
        $crate::record_flow_begin($category, $name, $flow_id as u64);
    }};
    ($name:expr, $flow_id:expr $(,)?) => {{
        $crate::record_flow_begin($crate::CATEGORY_APP_CUSTOM, $name, $flow_id as u64);
    }};
}

#[macro_export]
macro_rules! trace_flow_step {
    ($category:expr, $name:expr, $flow_id:expr $(,)?) => {{
        $crate::record_flow_step($category, $name, $flow_id as u64);
    }};
    ($name:expr, $flow_id:expr $(,)?) => {{
        $crate::record_flow_step($crate::CATEGORY_APP_CUSTOM, $name, $flow_id as u64);
    }};
}

#[macro_export]
macro_rules! trace_flow_end {
    ($category:expr, $name:expr, $flow_id:expr $(,)?) => {{
        $crate::record_flow_end($category, $name, $flow_id as u64);
    }};
    ($name:expr, $flow_id:expr $(,)?) => {{
        $crate::record_flow_end($crate::CATEGORY_APP_CUSTOM, $name, $flow_id as u64);
    }};
}
