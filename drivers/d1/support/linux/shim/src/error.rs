#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinuxError {
    Invalid,
    NoMemory,
    Io,
    Timeout,
    Unsupported,
}

pub type Result<T> = core::result::Result<T, LinuxError>;
