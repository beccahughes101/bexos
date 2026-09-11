use super::*;

pub(crate) struct KernelPlatformUpdateApplier {
    pub(crate) transport: KernelTransport,
    pub(crate) commit_pending: bool,
    pub(crate) awaiting_completion: bool,
    pub(crate) completion: Option<Result<u64, ()>>,
}

impl KernelPlatformUpdateApplier {
    pub(crate) fn new(transport: KernelTransport) -> Self {
        Self {
            transport,
            commit_pending: false,
            awaiting_completion: false,
            completion: None,
        }
    }

    pub(crate) fn commit_if_pending(&mut self) {
        if self.commit_pending {
            self.commit_pending = false;
            log("debugd: committing staged kernel transplant\n");
            bexos_userspace::syscall::commit_transplant();
            self.awaiting_completion = true;
        }
    }

    pub(crate) fn observe_completion(&mut self) {
        if !self.awaiting_completion {
            return;
        }
        if let Ok((bytes, handles)) = self.call_raw(
            "GetUpdateStatus",
            &KernelDebugControlGetUpdateStatusRequest {},
        ) {
            if let Ok(status) = KernelDebugControlGetUpdateStatusResponse::decode(&bytes, &handles)
            {
                if status.update_status == kernel_fidl::KernelUpdateStatus::Completed {
                    self.completion = Some(Ok(status.generation));
                    self.awaiting_completion = false;
                    log("debugd: resumed after kernel transplant\n");
                } else if status.update_status == kernel_fidl::KernelUpdateStatus::Failed {
                    self.completion = Some(Err(()));
                    self.awaiting_completion = false;
                    log("debugd: kernel preparation failed; old kernel still serving\n");
                }
            }
        }
    }

    pub(crate) fn call_raw<Q>(
        &mut self,
        method_name: &str,
        request: &Q,
    ) -> Result<(alloc::vec::Vec<u8>, alloc::vec::Vec<HandleRef>), kernel_fidl::FidlWireError>
    where
        Q: KernelEncode,
    {
        let ordinal = kernel_fidl::KERNEL_DEBUG_CONTROL_PUBLIC_METHODS
            .iter()
            .find(|method| method.name == method_name)
            .map(|method| method.ordinal)
            .ok_or(kernel_fidl::FidlWireError::Transport)?;
        let mut request_bytes = alloc::vec![0; 1024];
        let mut request_handles = [HandleRef { raw: 0 }; 1];
        let encoded = request.encode(&mut request_bytes, &mut request_handles)?;
        let mut response_bytes = alloc::vec![0; 1024];
        let mut response_handles = [HandleRef { raw: 0 }; 4];
        let received = kernel_fidl::FidlTransport::call(
            &mut self.transport,
            ordinal,
            &request_bytes[..encoded.bytes],
            &request_handles[..encoded.handles],
            &mut response_bytes,
            &mut response_handles,
        )?;
        Ok((
            response_bytes[..received.bytes].to_vec(),
            response_handles[..received.handles].to_vec(),
        ))
    }
}

impl PlatformUpdateApplier for KernelPlatformUpdateApplier {
    fn completion(&mut self) -> Option<Result<u64, ()>> {
        self.completion
    }
    fn external_update_started(&mut self) {
        self.completion = None;
        self.awaiting_completion = true;
    }
    async fn apply_platform_update(
        &mut self,
        manifest: &UpdateManifest,
        artifact: &[u8],
    ) -> bexos_debug_wire::DebugStatusResponse {
        let kind = match manifest.artifact_kind {
            ArtifactKind::Microkernel => KernelUpdateKind::Microkernel,
            ArtifactKind::TeeImage => KernelUpdateKind::TeeImage,
            ArtifactKind::Hypervisor => KernelUpdateKind::Hypervisor,
            ArtifactKind::AppPackage => {
                return debug_status(-8, "app updates do not use kernel platform path");
            }
        };
        let Ok(artifact_vmo) = Memory::from_bytes(artifact) else {
            return debug_status(-6, "kernel platform update artifact VMO failed");
        };
        let response = self.call_raw(
            "ApplyPlatformUpdate",
            &KernelDebugControlApplyPlatformUpdateRequest {
                kind,
                generation: manifest.generation,
                target: &manifest.target_id,
                artifact_hash: &manifest.artifact_hash,
                artifact_len: manifest.artifact_len,
                artifact: HandleRef { raw: artifact_vmo },
            },
        );
        let _ = Memory::close(artifact_vmo);
        let Ok((bytes, handles)) = response else {
            return debug_status(-6, "kernel platform update transport failed");
        };
        let Ok(response) =
            kernel_fidl::KernelDebugControlApplyPlatformUpdateResponse::decode(&bytes, &handles)
        else {
            return debug_status(-6, "kernel platform update decode failed");
        };
        if response.status == Status::Ok {
            self.completion = None;
            self.commit_pending = true;
        }
        debug_status(response.status as i32, response.message)
    }

    async fn platform_update_status(&mut self) -> bexos_debug_wire::DebugStatusResponse {
        let response = self.call_raw(
            "GetUpdateStatus",
            &KernelDebugControlGetUpdateStatusRequest {},
        );
        let Ok((bytes, handles)) = response else {
            return debug_status(-6, "kernel update status transport failed");
        };
        let decoded = KernelDebugControlGetUpdateStatusResponse::decode(&bytes, &handles);
        match decoded {
            Ok(response) => debug_status(
                response.status as i32,
                &alloc::format!(
                    "{:?} generation={} {}",
                    response.update_status,
                    response.generation,
                    response.message
                ),
            ),
            Err(_) => debug_status(-6, "kernel update status decode failed"),
        }
    }
}
