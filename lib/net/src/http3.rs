#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Http3Settings {
    pub qpack_max_table_capacity: u32,
    pub qpack_blocked_streams: u32,
}

impl Default for Http3Settings {
    fn default() -> Self {
        Self {
            qpack_max_table_capacity: 0,
            qpack_blocked_streams: 0,
        }
    }
}
