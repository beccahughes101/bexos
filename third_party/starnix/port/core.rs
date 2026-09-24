//! BexOS Phase 2 configuration of the vendored Starnix core boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Architecture {
    Aarch64,
    X86_64,
}

impl Architecture {
    pub const fn current() -> Self {
        if cfg!(bexos_arch_x86_64) {
            Self::X86_64
        } else {
            Self::Aarch64
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Syscall {
    Write,
    Exit,
    ExitGroup,
    Unsupported(u64),
}

impl Syscall {
    pub const fn decode(architecture: Architecture, number: u64) -> Self {
        match (architecture, number) {
            (Architecture::Aarch64, 64) | (Architecture::X86_64, 1) => Self::Write,
            (Architecture::Aarch64, 93) | (Architecture::X86_64, 60) => Self::Exit,
            (Architecture::Aarch64, 94) | (Architecture::X86_64, 231) => Self::ExitGroup,
            (_, number) => Self::Unsupported(number),
        }
    }
}

pub const ENOSYS: i64 = 38;
pub const EBADF: i64 = 9;
pub const EFAULT: i64 = 14;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Task {
    exit_code: Option<i32>,
}

impl Task {
    pub const fn new() -> Self {
        Self { exit_code: None }
    }

    pub fn exit(&mut self, code: u64) {
        self.exit_code = Some(code as u8 as i32);
    }

    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }
}

pub const fn error(errno: i64) -> u64 {
    (-errno) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syscall_numbers_match_linux_abis() {
        assert_eq!(Syscall::decode(Architecture::X86_64, 1), Syscall::Write);
        assert_eq!(Syscall::decode(Architecture::Aarch64, 64), Syscall::Write);
        assert_eq!(
            Syscall::decode(Architecture::X86_64, 231),
            Syscall::ExitGroup
        );
        assert_eq!(
            Syscall::decode(Architecture::Aarch64, 999),
            Syscall::Unsupported(999)
        );
    }

    #[test]
    fn exit_status_is_linux_low_byte() {
        let mut task = Task::new();
        task.exit(0x123);
        assert_eq!(task.exit_code(), Some(0x23));
        assert_eq!(error(ENOSYS) as i64, -38);
    }
}
