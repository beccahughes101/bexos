use core::time::Duration;

use crate::{LinuxError, Result};

pub async fn yield_now() {
    #[cfg(feature = "std")]
    bexos_userspace::yield_now();
}

pub async fn wait_until<F>(timeout: Duration, mut poll: F) -> Result<()>
where
    F: FnMut() -> Result<bool>,
{
    let start = now_ns();
    let timeout_ns = timeout.as_nanos().min(u64::MAX as u128) as u64;
    loop {
        if poll()? {
            return Ok(());
        }
        if now_ns().saturating_sub(start) > timeout_ns {
            return Err(LinuxError::Timeout);
        }
        yield_now().await;
    }
}

fn now_ns() -> u64 {
    #[cfg(feature = "std")]
    {
        let ticks = bexos_userspace::syscall::ticks();
        let frequency = bexos_userspace::syscall::frequency().max(1);
        return (u128::from(ticks) * 1_000_000_000u128 / u128::from(frequency)) as u64;
    }
    #[cfg(not(feature = "std"))]
    {
        0
    }
}
