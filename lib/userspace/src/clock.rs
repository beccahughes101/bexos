//! Direct clock reads report unavailable realtime instead of substituting uptime.
use crate::ipc::{check, kernel_call};
use kernel_fidl::{
    CLOCK_PUBLIC_METHODS, ClockGetTimeRequest, ClockGetTimeResponse, ClockType, Status,
};
pub fn realtime_ns() -> Result<u64, Status> {
    let response: ClockGetTimeResponse = kernel_call(
        8,
        "GetTime",
        CLOCK_PUBLIC_METHODS,
        &ClockGetTimeRequest {
            clock_type: ClockType::Realtime,
        },
    )?;
    check(response.status)?;
    Ok(response.nanos)
}
