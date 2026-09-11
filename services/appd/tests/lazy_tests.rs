use super::*;

fn heart_transplant_service_process(name: &str) -> bexos_appd::Process {
    bexos_appd::Process {
        name: name.into(),
        service: true,
        lifecycle: bexos_appd::ProcessLifecycle {
            update_strategy: bexos_appd::UpdateStrategy::HeartTransplant,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn lazy_keychain_service() -> ExposedService {
    ExposedService {
        name: "bexos.security.Keychain".into(),
        protocol: "Keychain".into(),
        lifecycle: Lifecycle::Singleton,
        visibility: Visibility::Public,
        activation: ServiceActivation::Lazy,
        capabilities: vec![CapabilityMetadata {
            capability: "Public".into(),
            permission: None,
            method_ordinals: vec![1, 2],
        }],
        ..Default::default()
    }
}

#[test]
fn lazy_manifest_infers_single_service_process_and_defaults_timeout() {
    let manifest = Manifest {
        package_name: "bexos.service.keychaind".into(),
        processes: vec![heart_transplant_service_process("keychaind")],
        services_exposed: vec![lazy_keychain_service()],
        ..Default::default()
    };

    manifest.validate_package_shape().unwrap();
    assert_eq!(
        manifest
            .service_provider_process(&manifest.services_exposed[0])
            .unwrap()
            .name,
        "keychaind"
    );
    assert_eq!(
        manifest
            .service_idle_timeout_ms(&manifest.services_exposed[0])
            .unwrap(),
        bexos_appd::DEFAULT_LAZY_IDLE_TIMEOUT_MS
    );
}

#[test]
fn lazy_manifest_rejects_ambiguous_or_restart_only_provider() {
    let mut ambiguous = Manifest {
        package_name: "com.example.lazy".into(),
        processes: vec![
            heart_transplant_service_process("a"),
            heart_transplant_service_process("b"),
        ],
        services_exposed: vec![lazy_keychain_service()],
        ..Default::default()
    };
    assert_eq!(
        ambiguous.validate_package_shape(),
        Err(bexos_appd::ManifestError::InvalidLazyService)
    );

    ambiguous.services_exposed[0].provider_process = Some("a".into());
    ambiguous.processes[0].lifecycle.update_strategy = bexos_appd::UpdateStrategy::Restart;
    assert_eq!(
        ambiguous.validate_package_shape(),
        Err(bexos_appd::ManifestError::InvalidLazyService)
    );
}

#[test]
fn dormant_broker_authorizes_without_live_manager() {
    let mut broker = AppdBroker::new();
    let mut service = lazy_keychain_service();
    service.provider_process = Some("keychaind".into());
    broker
        .publish_dormant_interface("bexos.service.keychaind", service, "keychaind")
        .unwrap();
    let consumed = ConsumedService {
        name: "bexos.security.Keychain".into(),
        link_type: LinkType::Required,
        capabilities: vec![ConsumedCapability {
            capability: "Public".into(),
            methods: vec![MethodDependency {
                ordinal: 1,
                link_type: LinkType::Required,
            }],
        }],
        ..Default::default()
    };
    let client = ClientContext {
        package_name: "com.example.client".into(),
        permissions: Vec::new(),
        permission_values: Vec::new(),
        user_id: Some(1000),
        is_foreground: true,
    };
    let mut kernel = FakeKernelOps::default();

    let binding = broker
        .bind_consumed_service(&client, &consumed, &mut kernel)
        .unwrap()
        .remove(0);
    assert_eq!(binding.activation, ServiceActivation::Lazy);
    assert_eq!(binding.provider_manager.object_id, 0);
    assert!(binding.dormant_provider);
    assert_eq!(binding.provider_process.as_deref(), Some("keychaind"));
}

#[test]
fn lazy_state_coalesces_and_bounds_pending_binds() {
    let mut state = bexos_appd::lazy::LazyActivationState::default();
    let mut binding = BoundCapability {
        caller_package: "client".into(),
        caller_uid: Some(1000),
        caller_foreground: true,
        provider_package: "provider".into(),
        provider_instance_id: None,
        service_name: "svc".into(),
        protocol: "Proto".into(),
        lifecycle: Lifecycle::Singleton,
        capability: "Public".into(),
        permission: None,
        method_ordinals: vec![1],
        permission_values: Vec::new(),
        provider_manager: Capability {
            object_id: 0,
            rights: 0,
        },
        client_endpoint: Capability {
            object_id: 1,
            rights: 3,
        },
        provider_endpoint: Capability {
            object_id: 2,
            rights: 3,
        },
        activation: ServiceActivation::Lazy,
        provider_process: Some("main".into()),
        idle_timeout_ms: 0,
        dormant_provider: true,
    };
    assert_eq!(
        state.demand(binding.clone(), 0).unwrap(),
        bexos_appd::lazy::LazyDemand::LaunchRequired
    );
    binding.provider_endpoint.object_id = 3;
    assert_eq!(
        state.demand(binding, 0).unwrap(),
        bexos_appd::lazy::LazyDemand::QueuedWhileStarting
    );
    assert_eq!(state.take_starting_pending("provider", "main", 0).len(), 2);
}
