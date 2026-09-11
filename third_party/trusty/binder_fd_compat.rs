//! Minimal owned-file-descriptor API for Android Binder on Trusty's custom
//! Rust target. Upstream Binder uses `std::os::fd`, which libstd only exports
//! for targets Rust classifies as Unix; Trusty supplies the same libc FD ABI
//! without setting that target-family cfg.

pub type RawFd = i32;

pub trait AsRawFd {
    fn as_raw_fd(&self) -> RawFd;
}

pub trait IntoRawFd {
    fn into_raw_fd(self) -> RawFd;
}

pub trait FromRawFd {
    unsafe fn from_raw_fd(fd: RawFd) -> Self;
}

#[derive(Debug)]
pub struct OwnedFd(RawFd);

impl AsRawFd for OwnedFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl IntoRawFd for OwnedFd {
    fn into_raw_fd(self) -> RawFd {
        let fd = self.0;
        core::mem::forget(self);
        fd
    }
}

impl FromRawFd for OwnedFd {
    unsafe fn from_raw_fd(fd: RawFd) -> Self {
        Self(fd)
    }
}

impl Drop for OwnedFd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            // Trusty's libc provides the POSIX-compatible close operation
            // used by the Binder NDK implementation.
            unsafe {
                libc::close(self.0);
            }
        }
    }
}
