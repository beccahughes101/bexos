use bexos_updated::service::UpdateService;
use update_manager_fidl::{
    UpdateManagerBeginUploadRequest, UpdateManagerCommitUploadRequest,
    UpdateManagerWriteChunkRequest, UpdateStatus, UpdateStream,
};

#[test]
fn stages_uploaded_manifest_and_artifact() {
    let mut service = UpdateService::new();
    assert_eq!(
        service
            .begin_upload(UpdateManagerBeginUploadRequest {
                upload_id: 7,
                manifest_len: 4,
                artifact_len: 3,
            })
            .status,
        UpdateStatus::Ok
    );
    assert_eq!(
        service
            .write_chunk(UpdateManagerWriteChunkRequest {
                upload_id: 7,
                stream: UpdateStream::Manifest,
                offset: 0,
                bytes: b"meta",
            })
            .status,
        UpdateStatus::Ok
    );
    assert_eq!(
        service
            .write_chunk(UpdateManagerWriteChunkRequest {
                upload_id: 7,
                stream: UpdateStream::Artifact,
                offset: 0,
                bytes: b"bin",
            })
            .status,
        UpdateStatus::Ok
    );
    assert_eq!(
        service
            .commit_upload(UpdateManagerCommitUploadRequest { upload_id: 7 })
            .status,
        UpdateStatus::Ok
    );
    let status = service.status_response();
    assert!(status.has_staged);
    assert!(!status.has_upload);
}
