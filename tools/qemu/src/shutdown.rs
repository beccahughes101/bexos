//! Convert developer-instance interrupts into normal cleanup of owned children.
use std::sync::atomic::{AtomicBool, Ordering};

static REQUESTED: AtomicBool = AtomicBool::new(false);
extern "C" fn interrupt(_: libc::c_int) {
    REQUESTED.store(true, Ordering::Relaxed);
}

pub(crate) fn requested() -> bool {
    REQUESTED.load(Ordering::Relaxed)
}

pub(crate) struct Signals(Vec<(libc::c_int, libc::sigaction)>);
impl Signals {
    pub(crate) fn install() -> Result<Self, String> {
        REQUESTED.store(false, Ordering::Relaxed);
        let mut guard = Self(Vec::new());
        for signal in [libc::SIGINT, libc::SIGTERM] {
            // sigaction is a C POD; an empty mask and flags are appropriate here.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = interrupt as *const () as libc::sighandler_t;
            unsafe {
                libc::sigemptyset(&mut action.sa_mask);
            }
            if unsafe { libc::sigaction(signal, &action, &mut previous) } != 0 {
                return Err(format!(
                    "install QEMU shutdown handler: {}",
                    std::io::Error::last_os_error()
                ));
            }
            guard.0.push((signal, previous));
        }
        Ok(guard)
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        for (signal, previous) in self.0.iter().rev() {
            unsafe {
                libc::sigaction(*signal, previous, std::ptr::null_mut());
            }
        }
        REQUESTED.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn interrupts_request_owned_child_cleanup() {
        let guard = super::Signals::install().unwrap();
        assert!(!super::requested());
        unsafe {
            libc::raise(libc::SIGINT);
        }
        assert!(super::requested());
        drop(guard);
        assert!(!super::requested());
    }
}
