use bexos_teed::{DriverBackend, SoftwareEmuBackend, TeeBackend, TeeService, migration::Runtime};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
};
use tee_manager_fidl::TeeStatus;

#[test]
fn proxy_endpoint_is_owned_and_restored_before_backend_activation() {
    let mut backend = DriverBackend::empty();
    backend.set_rpmb_channel(Channel(71));
    backend.restore_state(SoftwareEmuBackend::new());
    let mut source = Runtime::new(Channel(1), Some(Channel(2)), TeeService::new(backend));
    let record = source.encode_record(0).unwrap().unwrap();
    assert!(
        source
            .resources()
            .iter()
            .any(|r| matches!(r, Resource::Handle(71)))
    );

    let mut target = Runtime::<DriverBackend>::empty();
    target.adopt_record(0, Some(&record)).unwrap();
    target.validate().unwrap();
    assert_eq!(
        target.service.backend().rpmb_channel().map(|c| c.0),
        Some(71)
    );
    assert!(
        target
            .resources()
            .iter()
            .any(|r| matches!(r, Resource::Handle(71)))
    );

    // Preparing a candidate neither consumes nor alters the source's private
    // endpoint. Dropping an uncommitted candidate leaves source state intact.
    drop(target);
    source.service.backend_mut().pause_backend().unwrap();
    assert_eq!(
        source.service.backend().rpmb_channel().map(|c| c.0),
        Some(71)
    );
    assert_eq!(source.encode_record(0).unwrap().unwrap(), record);
}

#[test]
fn paused_transport_fails_requests_explicitly_instead_of_claiming_completion() {
    let mut backend = DriverBackend::empty();
    backend.set_rpmb_channel(Channel(71));
    backend.pause_backend().unwrap();
    super::test_runtime().block_on(async {
        assert_eq!(
            backend.list_trusted_apps().await,
            Err(TeeStatus::ErrPeerClosed)
        );
    });
}

#[test]
fn incomplete_or_trailing_proxy_records_are_rejected() {
    let mut backend = DriverBackend::empty();
    backend.set_rpmb_channel(Channel(71));
    let source = Runtime::new(Channel(1), Some(Channel(2)), TeeService::new(backend));
    let record = source.encode_record(0).unwrap().unwrap();
    let mut target = Runtime::<DriverBackend>::empty();
    assert!(
        target
            .adopt_record(0, Some(&record[..record.len() - 1]))
            .is_err()
    );
    let mut extra = record;
    extra.push(0);
    assert!(target.adopt_record(0, Some(&extra)).is_err());
}
