//! Optional queue timestamps. These measure a queue interval, never VSYNC or
//! pure shader busy time: submission gaps and backend scheduling can contribute.
pub const FEATURES: wgpu::Features =
    wgpu::Features::TIMESTAMP_QUERY.union(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);

pub struct Timestamps {
    queries: wgpu::QuerySet,
    resolved: wgpu::Buffer,
    period_ns: f32,
}

impl Timestamps {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        let period_ns = queue.get_timestamp_period();
        if !device.features().contains(FEATURES) || !period_ns.is_finite() || period_ns <= 0. {
            return None;
        }
        Some(Self {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("Flatland queue interval"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            }),
            resolved: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Flatland timestamp resolve"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            period_ns,
        })
    }

    pub fn begin(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.write_timestamp(&self.queries, 0);
        queue.submit([encoder.finish()]);
    }

    pub fn end(&self, encoder: &mut wgpu::CommandEncoder, readback: &wgpu::Buffer, offset: u64) {
        encoder.write_timestamp(&self.queries, 1);
        encoder.resolve_query_set(&self.queries, 0..2, &self.resolved, 0);
        encoder.copy_buffer_to_buffer(&self.resolved, 0, readback, offset, 16);
    }

    pub fn elapsed_ns(&self, bytes: &[u8]) -> Option<u64> {
        elapsed_ns(bytes, self.period_ns)
    }
}

/// Reject malformed, reversed/wrapped, or unusable counters instead of
/// publishing an invented duration. No absolute GPU/CPU clock comparison.
pub fn elapsed_ns(bytes: &[u8], period_ns: f32) -> Option<u64> {
    if !period_ns.is_finite() || period_ns <= 0. {
        return None;
    }
    let start = u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?);
    let end = u64::from_le_bytes(bytes.get(8..16)?.try_into().ok()?);
    let ns = end.checked_sub(start)? as f64 * f64::from(period_ns);
    (ns.is_finite() && ns < u64::MAX as f64).then_some(ns.round() as u64)
}
