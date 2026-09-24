use crate::{Result, Status};
use kernel_fidl::{ClockGetTimeRequest, ClockGetTimeResponse, ClockType};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockId {
    Monotonic,
    Boot,
    Realtime,
}

pub struct Clock;

impl Clock {
    pub fn get(id: ClockId) -> Result<u64> {
        let clock_type = match id {
            ClockId::Monotonic => ClockType::Monotonic,
            ClockId::Boot => ClockType::BootTime,
            ClockId::Realtime => ClockType::Realtime,
        };
        let response: ClockGetTimeResponse = bexos_userspace::ipc::kernel_call(
            8,
            "GetTime",
            kernel_fidl::CLOCK_PUBLIC_METHODS,
            &ClockGetTimeRequest { clock_type },
        )
        .map_err(Status::from)?;
        bexos_userspace::ipc::check(response.status).map_err(Status::from)?;
        Ok(response.nanos)
    }
}
