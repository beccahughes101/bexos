use super::*;

/// Keep a TEE session on its owning channel and close the binding on every exit.
pub(super) struct TeeManagerBinding(pub Channel);

impl TeeManagerBinding {
    pub fn new(teed: Channel) -> Result<Self, String> {
        bind_teed_manager(Some(teed), &[5, 6, 7])
            .map(Self)
            .map_err(|status| format!("TEE session binding {status:?}"))
    }
}

impl Drop for TeeManagerBinding {
    fn drop(&mut self) {
        let _ = Memory::close(self.0.0);
    }
}
