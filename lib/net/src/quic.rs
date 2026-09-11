#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuicEndpointConfig {
    pub max_idle_timeout_ms: u32,
    pub initial_max_data: u64,
    pub initial_max_stream_data: u64,
}

impl Default for QuicEndpointConfig {
    fn default() -> Self {
        Self {
            max_idle_timeout_ms: 30_000,
            initial_max_data: 1 << 20,
            initial_max_stream_data: 256 << 10,
        }
    }
}
