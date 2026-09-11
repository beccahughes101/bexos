//! Own the native WASM process until the standard startup handshake succeeds.
use super::{KernelOps, LaunchResult};
pub struct PendingWasmLaunch<'a, K: KernelOps> {
    kernel: &'a mut K,
    launch: Option<LaunchResult>,
}
impl<'a, K: KernelOps> PendingWasmLaunch<'a, K> {
    pub fn new(kernel: &'a mut K, launch: Option<LaunchResult>) -> Self {
        Self { kernel, launch }
    }
    pub fn startup_sent(&mut self) {
        if let Some(launch) = &mut self.launch {
            launch.runtime_linker_data = None;
        }
    }
    pub fn ready(&mut self) {
        self.launch = None;
    }
}
impl<K: KernelOps> Drop for PendingWasmLaunch<'_, K> {
    fn drop(&mut self) {
        if let Some(launch) = self.launch.take() {
            let _ = self.kernel.terminate_process(launch.process_handle, -1);
            if let Some((handle, _)) = launch.runtime_linker_data {
                let _ = self.kernel.release_vmo(handle);
            }
            for handle in [
                launch.service_manager_handle,
                launch.main_thread_handle,
                launch.address_space_handle,
                launch.process_handle,
            ] {
                let _ = self.kernel.close_handle(handle);
            }
        }
    }
}
