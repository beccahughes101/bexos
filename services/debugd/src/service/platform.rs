use super::*;

pub trait PlatformUpdateApplier {
    async fn apply_platform_update(
        &mut self,
        manifest: &UpdateManifest,
        artifact: &[u8],
    ) -> DebugStatusResponse;
    async fn platform_update_status(&mut self) -> DebugStatusResponse;
    /// None while staging/preparing; Some(Err) means a precommit abort.
    fn completion(&mut self) -> Option<Result<u64, ()>> {
        None
    }
    /// An external update manager staged a kernel replacement. Implementations
    /// can observe shared kernel status until this endpoint resumes.
    fn external_update_started(&mut self) {}
}

pub struct UnsupportedPlatformUpdateApplier;

impl PlatformUpdateApplier for UnsupportedPlatformUpdateApplier {
    async fn apply_platform_update(
        &mut self,
        _manifest: &UpdateManifest,
        _artifact: &[u8],
    ) -> DebugStatusResponse {
        DebugStatusResponse {
            status: -95,
            message: "kernel platform update backend unavailable".into(),
        }
    }

    async fn platform_update_status(&mut self) -> DebugStatusResponse {
        DebugStatusResponse {
            status: -95,
            message: "kernel platform update backend unavailable".into(),
        }
    }
}
