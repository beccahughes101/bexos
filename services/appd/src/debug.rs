use alloc::string::String;
use alloc::vec::Vec;
use app_debug_fidl::{
    AppDebugControlListProcessesRequest, AppDebugControlListProcessesResponse,
    AppDebugControlPublicServer, AppDebugStatus, AppProcessDebugInfo, AppProcessState,
    FidlWireError, WireVector,
};

use crate::{LaunchedProcess, Manifest, Process};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppDebugProcessState {
    Created,
    Running,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppDebugProcess {
    pub pid: u64,
    pub process_name: String,
    pub package_id: String,
    pub state: AppDebugProcessState,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppDebugProcessRegistry {
    processes: Vec<AppDebugProcess>,
    fidl_scratch: Vec<AppProcessDebugInfo<'static>>,
}

impl AppDebugProcessRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_created(&mut self, pid: u64, manifest: &Manifest, process: &Process) {
        self.processes.push(AppDebugProcess {
            pid,
            process_name: process.name.clone(),
            package_id: manifest.package_name.clone(),
            state: AppDebugProcessState::Created,
        });
    }

    pub fn record_launched(&mut self, launched: &LaunchedProcess<'_>) {
        self.record_created(
            launched.result.process_handle.raw,
            launched.process_ref.manifest,
            launched.process_ref.process,
        );
        if let Some(process) = self.processes.last_mut() {
            process.state = AppDebugProcessState::Running;
        }
    }

    pub fn processes(&self) -> &[AppDebugProcess] {
        &self.processes
    }

    fn fidl_processes(&mut self) -> &[AppProcessDebugInfo<'static>] {
        self.fidl_scratch.clear();
        self.fidl_scratch
            .extend(self.processes.iter().map(|process| AppProcessDebugInfo {
                pid: process.pid,
                process_name: unsafe {
                    core::mem::transmute::<&str, &'static str>(&process.process_name)
                },
                package_id: unsafe {
                    core::mem::transmute::<&str, &'static str>(&process.package_id)
                },
                state: match process.state {
                    AppDebugProcessState::Created => AppProcessState::Created,
                    AppDebugProcessState::Running => AppProcessState::Running,
                    AppDebugProcessState::Stopped => AppProcessState::Stopped,
                },
            }));
        &self.fidl_scratch
    }
}

impl AppDebugControlPublicServer for AppDebugProcessRegistry {
    fn list_processes<'a>(
        &mut self,
        _request: AppDebugControlListProcessesRequest,
    ) -> Result<AppDebugControlListProcessesResponse<'a>, FidlWireError> {
        Ok(AppDebugControlListProcessesResponse {
            status: AppDebugStatus::Ok,
            processes: WireVector::from_slice(unsafe {
                core::mem::transmute::<_, &'a [_]>(self.fidl_processes())
            }),
        })
    }
}
