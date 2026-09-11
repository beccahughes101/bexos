#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Http2Settings {
    pub initial_window_size: u32,
    pub max_concurrent_streams: u32,
}

impl Default for Http2Settings {
    fn default() -> Self {
        Self {
            initial_window_size: 65_535,
            max_concurrent_streams: 100,
        }
    }
}
