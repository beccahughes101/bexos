use super::{
    ControlPlane, Handle, KernelServiceStatus, RIGHT_DUPLICATE, RIGHT_MAP, RIGHT_READ,
    RIGHT_TRANSFER, handle::ObjectKind,
};
use bexos_time_abi::{SlewState, TimePageSnapshot};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockResult {
    pub status: KernelServiceStatus,
    pub nanos: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockState {
    monotonic_nanos: u64,
    slew: SlewState,
    vdso_vmo_id: u64,
}

impl ClockState {
    pub const fn new() -> Self {
        Self {
            monotonic_nanos: 0,
            slew: SlewState::new(),
            vdso_vmo_id: 0,
        }
    }

    pub fn set_monotonic_nanos(&mut self, nanos: u64) {
        if nanos > self.monotonic_nanos {
            self.monotonic_nanos = nanos;
        }
    }

    pub const fn monotonic_nanos(&self) -> u64 {
        self.monotonic_nanos
    }

    pub const fn realtime_offset_ns(&self) -> i64 {
        self.slew.realtime_offset_ns
    }

    pub const fn slew_start_monotonic_ns(&self) -> u64 {
        self.slew.slew_start_monotonic_ns
    }

    pub const fn slew_remaining_ns(&self) -> i64 {
        self.slew.slew_remaining_ns
    }

    pub const fn slew_rate_ppm(&self) -> i32 {
        self.slew.slew_rate_ppm
    }

    pub fn adjust_realtime(
        &mut self,
        offset_delta_ns: i64,
        slew_rate_ppm: i32,
    ) -> KernelServiceStatus {
        match self
            .slew
            .adjust(self.monotonic_nanos, offset_delta_ns, slew_rate_ppm)
        {
            Ok(slew) => {
                self.slew = slew;
                KernelServiceStatus::Ok
            }
            Err(_) => KernelServiceStatus::InvalidArgs,
        }
    }

    pub fn realtime_nanos(&self) -> Option<u64> {
        self.slew.realtime_at(self.monotonic_nanos)
    }

    pub const fn vdso_vmo_id(&self) -> u64 {
        self.vdso_vmo_id
    }

    pub fn set_vdso_vmo_id(&mut self, id: u64) {
        self.vdso_vmo_id = id;
    }

    pub fn time_page_snapshot(&self, base_ticks: u64, tick_frequency_hz: u64) -> TimePageSnapshot {
        TimePageSnapshot {
            base_ticks,
            tick_frequency_hz,
            base_monotonic_ns: self.monotonic_nanos,
            realtime_offset_ns: self.slew.realtime_offset_ns,
            slew_start_monotonic_ns: self.slew.slew_start_monotonic_ns,
            slew_remaining_ns: self.slew.slew_remaining_ns,
            slew_rate_ppm: self.slew.slew_rate_ppm,
        }
    }
}

impl Default for ClockState {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlPlane {
    pub fn set_monotonic_nanos_for_test(&mut self, nanos: u64) {
        self.clock.set_monotonic_nanos(nanos);
    }

    pub fn clock_get_time(&self, clock_type: kernel_fidl::ClockType) -> ClockResult {
        match clock_type {
            kernel_fidl::ClockType::Monotonic | kernel_fidl::ClockType::BootTime => ClockResult {
                status: KernelServiceStatus::Ok,
                nanos: self.clock.monotonic_nanos(),
            },
            kernel_fidl::ClockType::Realtime => match self.clock.realtime_nanos() {
                Some(nanos) => ClockResult {
                    status: KernelServiceStatus::Ok,
                    nanos,
                },
                None => ClockResult {
                    status: KernelServiceStatus::InvalidArgs,
                    nanos: 0,
                },
            },
        }
    }

    pub fn clock_adjust(
        &mut self,
        clock_type: kernel_fidl::ClockType,
        offset_delta_ns: i64,
        slew_rate_ppm: i32,
    ) -> KernelServiceStatus {
        match clock_type {
            kernel_fidl::ClockType::Realtime => {
                self.clock.adjust_realtime(offset_delta_ns, slew_rate_ppm)
            }
            kernel_fidl::ClockType::Monotonic | kernel_fidl::ClockType::BootTime => {
                KernelServiceStatus::InvalidArgs
            }
        }
    }

    pub fn clock_get_vdso_time_page(&mut self) -> Result<Handle, KernelServiceStatus> {
        let vmo_id = if self.clock.vdso_vmo_id() == 0 {
            let id = self.vmos.insert(4096, 0, None, 0, None)?;
            self.clock.set_vdso_vmo_id(id);
            id
        } else {
            self.clock.vdso_vmo_id()
        };
        self.handles.insert(
            vmo_id,
            ObjectKind::Vmo,
            RIGHT_READ | RIGHT_MAP | RIGHT_DUPLICATE | RIGHT_TRANSFER,
            0,
            None,
        )
    }
}
