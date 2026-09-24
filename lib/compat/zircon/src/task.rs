use crate::{Result, Status};
use bexos_userspace::restricted::{self, VectorEntry};
use kernel_fidl::{
    HandleRef, TaskControlFutexWaitRequest, TaskControlFutexWaitResponse,
    TaskControlFutexWakeRequest, TaskControlFutexWakeResponse,
};

pub struct Futex;

impl Futex {
    pub fn wait(address: u64, expected: u32, timeout_nanos: i64) -> Result<()> {
        let response: TaskControlFutexWaitResponse = bexos_userspace::ipc::kernel_call(
            3,
            "FutexWait",
            kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
            &TaskControlFutexWaitRequest {
                uaddr: address,
                expected_val: expected,
                timeout_nanos,
                owner_thread: HandleRef { raw: 0 },
            },
        )
        .map_err(Status::from)?;
        bexos_userspace::ipc::check(response.status).map_err(Status::from)
    }

    pub fn wake(address: u64, count: u32) -> Result<u32> {
        let response: TaskControlFutexWakeResponse = bexos_userspace::ipc::kernel_call(
            3,
            "FutexWake",
            kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
            &TaskControlFutexWakeRequest {
                uaddr: address,
                wake_count: count,
            },
        )
        .map_err(Status::from)?;
        bexos_userspace::ipc::check(response.status).map_err(Status::from)?;
        Ok(response.woken_count)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Thread(u64);

impl Thread {
    pub const unsafe fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub fn kick(&self) -> Result<()> {
        restricted::kick(self.0).map_err(Status::from)
    }
}

pub struct RestrictedState(bexos_userspace::restricted::RestrictedState);

impl RestrictedState {
    pub fn create() -> Result<Self> {
        bexos_userspace::restricted::RestrictedState::create()
            .map(Self)
            .map_err(Status::from)
    }

    pub const fn address(&self) -> u64 {
        self.0.address()
    }

    pub fn bind(&self) -> Result<()> {
        self.0.bind().map_err(Status::from)
    }

    pub fn unbind(&self) -> Result<()> {
        self.0.unbind().map_err(Status::from)
    }

    pub unsafe fn enter(&self, vector: VectorEntry, context: u64) -> Result<()> {
        unsafe { self.0.enter(vector, context) }.map_err(Status::from)
    }
}
