use super::*;

impl<T: DebugTransport> DebugClient<T> {
    pub fn install_test_app(
        &mut self,
        upload_id: u64,
        package_id: &str,
        manifest: &[u8],
        elf: &[u8],
    ) -> Result<(), DebugClientError> {
        self.begin_test_app_upload(
            upload_id,
            package_id,
            manifest.len() as u64,
            elf.len() as u64,
        )?;
        self.write_test_app_stream(upload_id, 1, manifest)?;
        self.write_test_app_stream(upload_id, 2, elf)?;
        self.commit_test_app_upload(upload_id)
    }

    pub fn upload_update(
        &mut self,
        upload_id: u64,
        manifest: &[u8],
        artifact: &[u8],
    ) -> Result<(), DebugClientError> {
        self.begin_update_upload(upload_id, manifest.len() as u64, artifact.len() as u64)?;
        self.write_update_stream(upload_id, 1, manifest)?;
        self.write_update_stream(upload_id, 2, artifact)?;
        self.commit_update_upload(upload_id)
    }

    pub fn launch_test_app(
        &mut self,
        package_id: &str,
        process_name: &str,
        arg0: u64,
    ) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_test_app_launch(
            &TestAppLaunchRequest {
                package_id: package_id.to_string(),
                process_name: process_name.to_string(),
                arg0,
            },
            &mut payload,
        );
        self.status_call(METHOD_LAUNCH_TEST_APP, payload)
    }

    pub fn begin_test_app_upload(
        &mut self,
        upload_id: u64,
        package_id: &str,
        manifest_len: u64,
        elf_len: u64,
    ) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_test_app_upload_begin(
            &TestAppUploadBeginRequest {
                upload_id,
                package_id: package_id.to_string(),
                manifest_len,
                elf_len,
            },
            &mut payload,
        );
        self.status_call(METHOD_BEGIN_TEST_APP_UPLOAD, payload)
    }

    pub(super) fn begin_app_bundle_upload(
        &mut self,
        upload_id: u64,
        archive_len: u64,
    ) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_app_bundle_upload_begin(
            &AppBundleUploadBeginRequest {
                upload_id,
                archive_len,
            },
            &mut payload,
        );
        self.status_call(METHOD_BEGIN_APP_BUNDLE_UPLOAD, payload)
    }

    pub(super) fn write_app_bundle_stream(
        &mut self,
        upload_id: u64,
        bytes: &[u8],
    ) -> Result<(), DebugClientError> {
        let max_chunk = 4 * 1024;
        for (index, chunk) in bytes.chunks(max_chunk).enumerate() {
            let mut payload = Vec::new();
            encode_app_bundle_chunk(
                &AppBundleChunkRequest {
                    upload_id,
                    offset: (index * max_chunk) as u64,
                    bytes: chunk.to_vec(),
                },
                &mut payload,
            );
            self.status_call(METHOD_WRITE_APP_BUNDLE_CHUNK, payload)?;
        }
        Ok(())
    }

    pub(super) fn commit_app_bundle_upload(
        &mut self,
        upload_id: u64,
    ) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_app_bundle_upload_commit(&AppBundleUploadCommitRequest { upload_id }, &mut payload);
        self.status_call(METHOD_COMMIT_APP_BUNDLE_UPLOAD, payload)
    }

    pub(super) fn begin_update_upload(
        &mut self,
        upload_id: u64,
        manifest_len: u64,
        artifact_len: u64,
    ) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_update_upload_begin(
            &UpdateUploadBeginRequest {
                upload_id,
                manifest_len,
                artifact_len,
            },
            &mut payload,
        );
        self.status_call(METHOD_BEGIN_UPDATE_UPLOAD, payload)
    }

    pub(super) fn write_update_stream(
        &mut self,
        upload_id: u64,
        stream: u32,
        bytes: &[u8],
    ) -> Result<(), DebugClientError> {
        // Leave room for the protobuf envelope under the 64 KiB debug frame
        // limit. Firmware uploads otherwise spend thousands of round trips
        // transferring a single image before preparation can even begin.
        let max_chunk = 60 * 1024;
        let window = 1;
        let mut requests = Vec::new();
        for (index, chunk) in bytes.chunks(max_chunk).enumerate() {
            let mut payload = Vec::new();
            encode_update_chunk(
                &UpdateChunkRequest {
                    upload_id,
                    stream,
                    offset: (index * max_chunk) as u64,
                    bytes: chunk.to_vec(),
                },
                &mut payload,
            );
            let request_id = self.send_frame(METHOD_WRITE_UPDATE_CHUNK, payload)?;
            requests.push((request_id, index));
            if requests.len() == window {
                self.read_update_acks(stream, &mut requests)?;
            }
        }
        self.read_update_acks(stream, &mut requests)
    }

    pub(super) fn commit_update_upload(&mut self, upload_id: u64) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_update_upload_commit(&UpdateUploadCommitRequest { upload_id }, &mut payload);
        self.status_call(METHOD_COMMIT_UPDATE_UPLOAD, payload)
    }

    pub(super) fn write_test_app_stream(
        &mut self,
        upload_id: u64,
        stream: u32,
        bytes: &[u8],
    ) -> Result<(), DebugClientError> {
        self.write_test_app_stream_from(upload_id, stream, 0, bytes)
    }

    pub fn write_test_app_stream_from(
        &mut self,
        upload_id: u64,
        stream: u32,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), DebugClientError> {
        let max_chunk = 48 * 1024;
        for (index, chunk) in bytes.chunks(max_chunk).enumerate() {
            let mut payload = Vec::new();
            encode_test_app_chunk(
                &TestAppChunkRequest {
                    upload_id,
                    stream,
                    offset: offset + (index * max_chunk) as u64,
                    bytes: chunk.to_vec(),
                },
                &mut payload,
            );
            self.status_call(METHOD_WRITE_TEST_APP_CHUNK, payload)?;
        }
        Ok(())
    }

    pub fn commit_test_app_upload(&mut self, upload_id: u64) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_test_app_upload_commit(&TestAppUploadCommitRequest { upload_id }, &mut payload);
        self.status_call(METHOD_COMMIT_TEST_APP_UPLOAD, payload)
    }
}
