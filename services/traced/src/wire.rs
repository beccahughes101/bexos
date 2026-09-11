use alloc::vec::Vec;
use bexos_trace::{
    BufferMode as TraceBufferMode, TraceOutputFormat as NativeTraceOutputFormat, TraceState,
};
use bexos_userspace::Channel;
use kernel_fidl::Status;
use tracing_fidl::{
    BufferMode, FidlEncode, HandleRef, TraceCategory, TraceControllerGetStatusResponse,
    TraceOutputFormat, TraceSessionState,
};

use crate::manager::{TraceError, TraceStatus};

pub fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[..8]);
    (u64::from_le_bytes(raw), &bytes[8..])
}

pub fn metadata_protocol(metadata: &str) -> Option<&str> {
    metadata.split('|').nth(1)
}

pub fn handle_refs(handles: &[u64]) -> Vec<HandleRef> {
    handles.iter().map(|raw| HandleRef { raw: *raw }).collect()
}

pub fn send_response<T: FidlEncode>(channel: Channel, response: &T) {
    let mut out = [0; 64];
    let mut handles = [HandleRef { raw: 0 }; 4];
    if let Ok(encoded) = response.encode(&mut out, &mut handles) {
        let raw: Vec<_> = handles[..encoded.handles].iter().map(|h| h.raw).collect();
        let _ = channel.send(&out[..encoded.bytes], &raw);
    }
}

pub fn trace_status_response(
    status: Status,
    trace: TraceStatus,
) -> TraceControllerGetStatusResponse {
    TraceControllerGetStatusResponse {
        status,
        state: trace_state(trace.state),
        categories: TraceCategory(trace.categories),
        buffer_mode: buffer_mode(trace.buffer_mode),
        buffer_size_kb: trace.buffer_size_kb,
        output_format: output_format(trace.output_format),
        producer_count: trace.producer_count,
        event_count: trace.event_count,
        dropped_count: trace.dropped_count,
    }
}

pub const fn trace_state(state: TraceState) -> TraceSessionState {
    match state {
        TraceState::Idle => TraceSessionState::Idle,
        TraceState::Recording => TraceSessionState::Recording,
        TraceState::Stopped => TraceSessionState::Stopped,
    }
}

pub const fn buffer_mode(mode: TraceBufferMode) -> BufferMode {
    match mode {
        TraceBufferMode::OneshotStopOnFull => BufferMode::OneshotStopOnFull,
        TraceBufferMode::CircularRing => BufferMode::CircularRing,
    }
}

pub const fn trace_buffer_mode(mode: BufferMode) -> TraceBufferMode {
    match mode {
        BufferMode::OneshotStopOnFull => TraceBufferMode::OneshotStopOnFull,
        BufferMode::CircularRing => TraceBufferMode::CircularRing,
    }
}

pub const fn output_format(format: NativeTraceOutputFormat) -> TraceOutputFormat {
    match format {
        NativeTraceOutputFormat::Perfetto => TraceOutputFormat::Perfetto,
        NativeTraceOutputFormat::LegacyBexosFxt => TraceOutputFormat::LegacyBexosFxt,
    }
}

pub const fn trace_output_format(format: TraceOutputFormat) -> NativeTraceOutputFormat {
    match format {
        TraceOutputFormat::Perfetto => NativeTraceOutputFormat::Perfetto,
        TraceOutputFormat::LegacyBexosFxt => NativeTraceOutputFormat::LegacyBexosFxt,
    }
}

pub const fn status_from_error(error: TraceError) -> Status {
    match error {
        TraceError::InvalidArgs => Status::ErrInvalidArgs,
        TraceError::AlreadyRecording => Status::ErrAlreadyExists,
        TraceError::AlreadyExists => Status::ErrAlreadyExists,
        TraceError::NotRecording => Status::ErrTimedOut,
        TraceError::NotFound => Status::ErrInvalidHandle,
    }
}
