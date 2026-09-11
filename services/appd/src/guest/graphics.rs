//! Optional boot progress must never become a dependency of storage or normal boot.
use bexos_userspace::{Channel, Memory};
use graphics_fidl::*;
pub struct BootGraphics {
    progress: Option<Channel>,
    scene: Option<Channel>,
}
impl BootGraphics {
    pub fn connect(gate: &super::readiness::Gate) -> Self {
        let progress = gate
            .notified_grant(
                "bexos.splash.ProgressTracker",
                "ProgressTracker",
                "Public",
                &[1],
                gate.splash,
            )
            .ok()
            .map(|g| Channel(g.endpoint));
        Self {
            progress,
            scene: gate.scened,
        }
    }
    pub fn report(&mut self, stage: u8, percent: u8, message: &str) {
        let Some(c) = self.progress.as_mut() else {
            return;
        };
        let stage = match stage {
            1 => BootStage::KernelBootstrap,
            2 => BootStage::StorageUnlocked,
            3 => BootStage::DriversInitialized,
            4 => BootStage::SystemAppsOnline,
            _ => BootStage::ReadyForCompositor,
        };
        let _: Result<ProgressTrackerReportStageResponse, _> = bexos_graphics_runtime::call(
            c,
            1,
            &ProgressTrackerReportStageRequest {
                stage,
                progress_pct: percent,
                status_message: message,
            },
        );
    }
    pub fn active(&self) -> bool {
        self.progress.is_some() || self.scene.is_some()
    }

    pub fn ready(&self) {
        if let Some(c) = self.scene {
            let _ = c.send(b"bexos.graphics.ready", &[]);
        }
    }
}
impl Drop for BootGraphics {
    fn drop(&mut self) {
        if let Some(c) = self.progress {
            let _ = Memory::close(c.0);
        }
    }
}

pub fn apply_render_profile(thread: u64, profile: u64) -> Result<(), kernel_fidl::Status> {
    use kernel_fidl::*;
    let result: Result<TaskControlSetProfileResponse, _> = bexos_userspace::ipc::kernel_call(
        3,
        "SetProfile",
        TASK_CONTROL_PUBLIC_METHODS,
        &TaskControlSetProfileRequest {
            thread: HandleRef { raw: thread },
            profile: HandleRef { raw: profile },
        },
    );
    let _ = Memory::close(profile);
    let response = result?;
    if response.status == Status::Ok {
        bexos_userspace::log("appd: graphics deadline profile applied\n");
        Ok(())
    } else {
        Err(response.status)
    }
}
