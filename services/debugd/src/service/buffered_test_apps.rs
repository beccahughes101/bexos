use super::*;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BufferedTestAppInstaller {
    pub(super) upload: Option<BufferedUpload>,
    pub(super) installed: Option<InstalledTestApp>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct BufferedUpload {
    pub(super) upload_id: u64,
    pub(super) package_id: String,
    pub(super) manifest_len: usize,
    pub(super) elf_len: usize,
    pub(super) manifest: Vec<u8>,
    pub(super) elf: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct InstalledTestApp {
    pub(super) package_id: String,
    pub(super) manifest: Vec<u8>,
    pub(super) elf: Vec<u8>,
}

impl BufferedTestAppInstaller {
    pub fn new() -> Self {
        Self::default()
    }

    fn target_stream(upload: &mut BufferedUpload, stream: u32) -> Option<(&mut Vec<u8>, usize)> {
        match stream {
            1 => Some((&mut upload.manifest, upload.manifest_len)),
            2 => Some((&mut upload.elf, upload.elf_len)),
            _ => None,
        }
    }
}

impl TestAppInstaller for BufferedTestAppInstaller {
    async fn begin_upload(&mut self, request: TestAppUploadBeginRequest) -> DebugStatusResponse {
        if request.package_id.is_empty()
            || request.manifest_len == 0
            || request.elf_len == 0
            || request.manifest_len > 256 * 1024
            || request.elf_len > 16 * 1024 * 1024
        {
            return invalid_request_response("invalid test-app upload dimensions");
        }
        self.upload = Some(BufferedUpload {
            upload_id: request.upload_id,
            package_id: request.package_id,
            manifest_len: request.manifest_len as usize,
            elf_len: request.elf_len as usize,
            manifest: Vec::new(),
            elf: Vec::new(),
        });
        ok_response("upload started")
    }

    async fn write_chunk(&mut self, request: TestAppChunkRequest) -> DebugStatusResponse {
        let Some(upload) = self.upload.as_mut() else {
            return invalid_request_response("no active test-app upload");
        };
        if upload.upload_id != request.upload_id {
            return invalid_request_response("test-app upload id mismatch");
        }
        let Some((target, expected_len)) = Self::target_stream(upload, request.stream) else {
            return invalid_request_response("invalid test-app upload stream");
        };
        if request.offset as usize != target.len() {
            return invalid_request_response("out-of-order test-app chunk");
        }
        if target.len().saturating_add(request.bytes.len()) > expected_len {
            return invalid_request_response("test-app chunk exceeds declared length");
        }
        target.extend_from_slice(&request.bytes);
        ok_response("chunk accepted")
    }

    async fn commit_upload(&mut self, request: TestAppUploadCommitRequest) -> DebugStatusResponse {
        let Some(upload) = self.upload.take() else {
            return invalid_request_response("no active test-app upload");
        };
        if upload.upload_id != request.upload_id {
            self.upload = Some(upload);
            return invalid_request_response("test-app upload id mismatch");
        }
        if upload.manifest.len() != upload.manifest_len || upload.elf.len() != upload.elf_len {
            self.upload = Some(upload);
            return invalid_request_response("test-app upload incomplete");
        }
        self.installed = Some(InstalledTestApp {
            package_id: upload.package_id,
            manifest: upload.manifest,
            elf: upload.elf,
        });
        ok_response("test app installed")
    }

    async fn launch(&mut self, request: TestAppLaunchRequest) -> DebugStatusResponse {
        let Some(installed) = self.installed.as_ref() else {
            return invalid_request_response("test app is not installed");
        };
        if installed.package_id != request.package_id || request.process_name.is_empty() {
            return invalid_request_response("unknown test app process");
        }
        DebugStatusResponse {
            status: -95,
            message: "debugd accepted the test app, but appd launch integration is unavailable"
                .into(),
        }
    }
}
