use super::*;

pub trait TestAppInstaller {
    async fn begin_upload(&mut self, request: TestAppUploadBeginRequest) -> DebugStatusResponse;
    async fn write_chunk(&mut self, request: TestAppChunkRequest) -> DebugStatusResponse;
    async fn commit_upload(&mut self, request: TestAppUploadCommitRequest) -> DebugStatusResponse;
    async fn launch(&mut self, request: TestAppLaunchRequest) -> DebugStatusResponse;
}

pub struct UnsupportedTestAppInstaller;

impl TestAppInstaller for UnsupportedTestAppInstaller {
    async fn begin_upload(&mut self, _request: TestAppUploadBeginRequest) -> DebugStatusResponse {
        unsupported_install_response()
    }

    async fn write_chunk(&mut self, _request: TestAppChunkRequest) -> DebugStatusResponse {
        unsupported_install_response()
    }

    async fn commit_upload(&mut self, _request: TestAppUploadCommitRequest) -> DebugStatusResponse {
        unsupported_install_response()
    }

    async fn launch(&mut self, _request: TestAppLaunchRequest) -> DebugStatusResponse {
        unsupported_install_response()
    }
}
