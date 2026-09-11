//! Allocation-free rolling measurements. Units and clock domains are chosen by
//! the caller; these statistics never equate submission completion with VSYNC.
pub const SAMPLES: usize = 256;
pub struct Window {
    values: [u64; SAMPLES],
    count: usize,
    next: usize,
}
impl Default for Window {
    fn default() -> Self {
        Self {
            values: [0; SAMPLES],
            count: 0,
            next: 0,
        }
    }
}
impl Window {
    pub fn record(&mut self, value: u64) {
        self.values[self.next] = value;
        self.next = (self.next + 1) % SAMPLES;
        self.count = (self.count + 1).min(SAMPLES);
    }
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    /// Nearest-rank percentile over the most recent 256 observations. The
    /// temporary sort uses fixed stack storage and does not disturb the ring.
    pub fn percentile(&self, percent: usize) -> Option<u64> {
        if self.count == 0 || percent == 0 || percent > 100 {
            return None;
        }
        let mut values = self.values;
        values[..self.count].sort_unstable();
        Some(values[(self.count * percent).div_ceil(100) - 1])
    }
}
