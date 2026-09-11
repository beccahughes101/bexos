use alloc::string::{String, ToString};

use crate::CATEGORY_DEBUG_SERVICE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceEventKind {
    Metadata,
    SliceBegin,
    SliceEnd,
    Instant,
    FlowBegin,
    FlowStep,
    FlowEnd,
    Counter,
    DroppedEvents,
}

impl TraceEventKind {
    pub const fn code(self) -> u8 {
        match self {
            Self::Metadata => 1,
            Self::SliceBegin => 2,
            Self::SliceEnd => 3,
            Self::Instant => 4,
            Self::FlowBegin => 5,
            Self::FlowStep => 6,
            Self::FlowEnd => 7,
            Self::Counter => 8,
            Self::DroppedEvents => 9,
        }
    }

    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Metadata),
            2 => Some(Self::SliceBegin),
            3 => Some(Self::SliceEnd),
            4 => Some(Self::Instant),
            5 => Some(Self::FlowBegin),
            6 => Some(Self::FlowStep),
            7 => Some(Self::FlowEnd),
            8 => Some(Self::Counter),
            9 => Some(Self::DroppedEvents),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceAnnotation {
    pub key: String,
    pub value: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceEvent {
    pub timestamp_ns: u64,
    pub pid: u64,
    pub tid: u64,
    pub category: u32,
    pub kind: TraceEventKind,
    pub name: String,
    pub flow_id: u64,
    pub value: i64,
    pub annotations: [Option<TraceAnnotation>; 2],
}

impl TraceEvent {
    pub fn metadata(timestamp_ns: u64, name: &str) -> Self {
        Self::new(
            timestamp_ns,
            0,
            0,
            CATEGORY_DEBUG_SERVICE,
            TraceEventKind::Metadata,
            name,
        )
    }

    pub fn new(
        timestamp_ns: u64,
        pid: u64,
        tid: u64,
        category: u32,
        kind: TraceEventKind,
        name: &str,
    ) -> Self {
        Self {
            timestamp_ns,
            pid,
            tid,
            category,
            kind,
            name: name.to_string(),
            flow_id: 0,
            value: 0,
            annotations: [None, None],
        }
    }

    pub fn flow(mut self, flow_id: u64) -> Self {
        self.flow_id = flow_id;
        self
    }

    pub fn counter(mut self, value: i64) -> Self {
        self.value = value;
        self
    }

    pub fn annotation(mut self, index: usize, key: &str, value: i64) -> Self {
        if index < self.annotations.len() {
            self.annotations[index] = Some(TraceAnnotation {
                key: key.to_string(),
                value,
            });
        }
        self
    }
}
