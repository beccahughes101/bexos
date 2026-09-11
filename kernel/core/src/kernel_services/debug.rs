use kernel_fidl::{DebugProcessState, KernelProcessDebugInfo};

use super::system::ProcessState;
use super::{ControlPlane, KernelServiceStatus};

impl ControlPlane {
    pub fn list_process_debug_info(
        &mut self,
    ) -> Result<&[KernelProcessDebugInfo], KernelServiceStatus> {
        self.scratch.process_debug.clear();
        self.scratch
            .process_debug
            .try_reserve_exact(self.processes.len())
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        self.scratch
            .process_debug
            .extend(self.processes.iter().map(|process| {
                let group = self.resource_groups.get(process.resource_group_id);
                let parent = group
                    .and_then(|g| g.parent_id)
                    .and_then(|id| self.resource_groups.get(id));
                KernelProcessDebugInfo {
                    pid: process.id,
                    main_thread_id: process.main_thread_id,
                    resource_group_id: process.resource_group_id,
                    resource_group_name_len: group.map_or(0, |g| g.name_len as u32),
                    resource_group_name: group.map_or([0; 24], |g| g.name),
                    parent_resource_group_id: parent.map_or(0, |g| g.id),
                    name_len: process.name_len as u32,
                    name: process.name,
                    package_id_len: process.package_id_len as u32,
                    package_id: process.package_id,
                    state: match process.state {
                        ProcessState::Created => DebugProcessState::Created,
                        ProcessState::Running => DebugProcessState::Running,
                        ProcessState::Suspended => DebugProcessState::Suspended,
                        ProcessState::Exited => DebugProcessState::Exited,
                    },
                }
            }));
        Ok(&self.scratch.process_debug)
    }
}
