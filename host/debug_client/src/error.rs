use super::*;

#[derive(Debug)]
pub enum DebugClientError {
    Io(std::io::Error),
    Wire(WireError),
    MethodMismatch { expected: u32, actual: u32 },
    RemoteStatus(DebugStatusResponse),
}

impl From<std::io::Error> for DebugClientError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<WireError> for DebugClientError {
    fn from(error: WireError) -> Self {
        Self::Wire(error)
    }
}

impl std::fmt::Display for DebugClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "debug transport: {e}"),
            Self::Wire(e) => write!(f, "debug protocol: {e}"),
            Self::MethodMismatch { expected, actual } => {
                write!(f, "debug response method {actual}, expected {expected}")
            }
            Self::RemoteStatus(s) => write!(f, "{} (status {})", s.message, s.status),
        }
    }
}
impl std::error::Error for DebugClientError {}
