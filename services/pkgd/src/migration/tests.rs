use super::*;
use bexos_pkg_config::{Consumer, Repository};

fn runtime() -> Runtime {
    let mut runtime = Runtime::empty();
    runtime.control = Channel(10);
    runtime.cache_file = Channel(11);
    runtime.state_file = Channel(12);
    runtime.directory = Channel(13);
    runtime.config.repositories.push(Repository {
        host: "registry.test".into(),
        repository: "apps/demo".into(),
        ..Repository::default()
    });
    runtime.config.consumers.push(Consumer {
        package: "test.consumer".into(),
        kinds: vec![ArtifactKind::Application as u32],
        repositories: vec!["registry.test/apps/demo".into()],
    });
    runtime.clients.push(Client {
        channel: 20,
        package: "test.consumer".into(),
        protocol: "PackageResolver".into(),
        methods: vec![1, 2],
    });
    runtime.jobs.push(Job {
        query: ArtifactQuery {
            registry_host: "registry.test".into(),
            repository: "apps/demo".into(),
            tag: "stable".into(),
            kind: ArtifactKind::Application,
            expected_digest: None,
        },
        source_digest: None,
        waiters: vec![Waiter {
            channel: 20,
            caller: "test.consumer".into(),
            expected: Some(BlobDigest {
                hash_type: HashType::Blake3,
                digest: [7; 32],
            }),
        }],
        attempt: 1,
        future: None,
    });
    runtime.blobs.insert(
        [9; 32],
        Blob {
            handle: 30,
            length: 4096,
        },
    );
    runtime.secrets.insert(
        "registry.test".into(),
        SecretHandle {
            handle: 40,
            length: 128,
            sealed: false,
        },
    );
    runtime
}

fn adopt(source: &Runtime, omitted: Option<u64>) -> Runtime {
    let mut target = Runtime::empty();
    target.config = source.config.clone();
    for key in source.keys() {
        if omitted != Some(key) {
            let bytes = source.encode_record(key).unwrap().unwrap();
            target.adopt_record(key, Some(&bytes)).unwrap();
        }
    }
    target
}

#[test]
fn checkpoint_preserves_authorization_constraints_and_private_resource_identity() {
    let source = runtime();
    let target = adopt(&source, None);
    target.validate().unwrap();
    assert_eq!(target.clients[0].package, "test.consumer");
    assert_eq!(target.clients[0].methods, [1, 2]);
    assert_eq!(
        target.jobs[0].waiters[0].expected,
        source.jobs[0].waiters[0].expected
    );
    assert_eq!(target.jobs[0].attempt, 1);
    assert_eq!(target.jobs[0].query, source.jobs[0].query);
    assert!(target.jobs[0].future.is_none());
    assert_eq!(target.blobs[&[9; 32]].handle, 30);
    assert_eq!(target.secrets["registry.test"].handle, 40);
    assert_eq!(target.cache_file.0, 11);
    assert_eq!(target.state_file.0, 12);
    assert_eq!(target.directory.0, 13);
}

#[test]
fn checkpoint_excludes_plaintext_credentials() {
    let mut source = runtime();
    let secret = b"private-token-must-not-be-checkpointed";
    let key = b"private-key-must-not-be-checkpointed";
    source
        .vault
        .set_token("registry.test", secret.to_vec(), false)
        .unwrap();
    source
        .vault
        .entries
        .get_mut("registry.test")
        .unwrap()
        .private_key = crate::credentials::Secret::new(key.to_vec());
    for key in source.keys() {
        let bytes = source.encode_record(key).unwrap().unwrap();
        assert!(!bytes.windows(secret.len()).any(|window| window == secret));
        let private_key = source
            .vault
            .get("registry.test")
            .unwrap()
            .private_key
            .bytes();
        assert!(
            !bytes
                .windows(private_key.len())
                .any(|window| window == private_key)
        );
    }
    // Only the private credential VMO's identity is part of serialized state.
    assert_eq!(adopt(&source, None).secrets["registry.test"].handle, 40);
}

#[test]
fn adoption_rejects_missing_inventory_and_request_authority() {
    let source = runtime();
    for omitted in [JOBS, BLOBS, SECRETS] {
        assert!(adopt(&source, Some(omitted)).validate().is_err());
    }
    for defect in 0..5 {
        let mut target = adopt(&source, None);
        match defect {
            0 => target.jobs[0].waiters[0].caller = "another.consumer".into(),
            1 => target.clients[0].methods = vec![2],
            2 => target.clients[0].protocol = "CredentialManager".into(),
            3 => target.clients[0].methods = vec![1, 99],
            _ => target.jobs[0].query.kind = ArtifactKind::Firmware,
        }
        assert!(target.validate().is_err(), "accepted defect {defect}");
    }
    source.validate().unwrap();
    let mut cached_source = runtime();
    cached_source.jobs[0].source_digest = cached_source.jobs[0].waiters[0].expected;
    cached_source.clients[0].methods = vec![2];
    adopt(&cached_source, None).validate().unwrap();
}

#[test]
fn quiescence_cancels_transport_and_retains_source_recovery_state() {
    use std::{cell::Cell, rc::Rc};
    struct Pending(Rc<Cell<bool>>);
    impl core::future::Future for Pending {
        type Output = crate::Result<crate::resolution::Prepared>;
        fn poll(
            self: core::pin::Pin<&mut Self>,
            _: &mut core::task::Context<'_>,
        ) -> core::task::Poll<Self::Output> {
            core::task::Poll::Pending
        }
    }
    impl Drop for Pending {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    let mut source = runtime();
    let dropped = Rc::new(Cell::new(false));
    source.jobs[0].future = Some(Box::pin(Pending(dropped.clone())));
    assert!(!source.quiescence_ready());
    source.pause();
    assert!(dropped.get());
    assert!(source.quiescence_ready());
    assert_eq!(source.jobs.len(), 1);
    assert!(adopt(&source, Some(BLOBS)).validate().is_err());
    source.validate().unwrap();
    assert_eq!(source.cache_file.0, 11);
    assert_eq!(source.blobs[&[9; 32]].handle, 30);
}
