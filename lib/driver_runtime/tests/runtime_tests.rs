use bexos_driver_runtime::*;

fn policy(colocation: ColocationPolicy, maximum: u32) -> HostPolicy {
    HostPolicy {
        package_id: "driver.test".into(),
        process_name: "driver".into(),
        signed_package_digest: [7; 32],
        colocation,
        max_instances: maximum,
    }
}

#[test]
fn validates_resources_and_isolation() {
    let context = BindContext {
        node_id: 1,
        parent_node_id: None,
        topological_path: "pci/0000:00:01.0".into(),
        resources: vec![Resource {
            kind: ResourceKind::IommuDomain,
            resource_id: 1,
            base: 1,
            length: 1,
            flags: 0,
            handle: 9,
        }],
    };
    context
        .validate(&[ResourceRequirement {
            kind: ResourceKind::IommuDomain,
            minimum: 1,
        }])
        .unwrap();
    assert_eq!(
        context.validate(&[ResourceRequirement {
            kind: ResourceKind::Interrupt,
            minimum: 1,
        }]),
        Err(RuntimeError::MissingResource(ResourceKind::Interrupt))
    );
    let mut host = HostMembership::new(11, policy(ColocationPolicy::Isolated, 1)).unwrap();
    host.bind(&host.policy.clone(), 1, None).unwrap();
    assert_eq!(
        host.bind(&host.policy.clone(), 2, None),
        Err(RuntimeError::Capacity)
    );
}

#[test]
fn host_shared_requires_ready_parent_and_same_signed_package() {
    let root = policy(ColocationPolicy::Isolated, 1);
    let mut host = HostMembership::new(11, root.clone()).unwrap();
    host.bind(&root, 99, None).unwrap();
    host.set_ready(99).unwrap();
    let shared = policy(ColocationPolicy::HostShared, 3);
    host.bind(&shared, 2, Some(99)).unwrap();
    let mut other = shared;
    other.signed_package_digest = [8; 32];
    assert_eq!(
        host.bind(&other, 3, Some(99)),
        Err(RuntimeError::PolicyMismatch)
    );
}

#[test]
fn migration_preserves_atomic_membership() {
    let mut host = HostMembership::new(11, policy(ColocationPolicy::Colocated, 2)).unwrap();
    host.bind(&host.policy.clone(), 1, None).unwrap();
    host.set_ready(1).unwrap();
    host.bind(&host.policy.clone(), 2, None).unwrap();
    host.set_ready(2).unwrap();
    host.quiesce_all();
    let encoded = host.encode_migration_record(42);
    let (generation, mut restored) = HostMembership::decode_migration_record(&encoded).unwrap();
    assert_eq!(generation, 42);
    restored.begin_restore_all().unwrap();
    assert!(
        restored
            .nodes()
            .iter()
            .all(|node| node.phase == NodePhase::Restoring)
    );
}
