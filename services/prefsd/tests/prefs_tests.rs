use bexos_component_config::{schema::*, transaction::Phase};
use bexos_prefsd::{binding::Client, runtime::Runtime, service::*};
use bexos_userspace::{Channel, live_migration::State, service_binding::ServiceBinding};
use std::collections::BTreeMap;
fn service() -> Service {
    let mut s = Service::default();
    let schema = Schema {
        fields: vec![Field {
            name: "dark".into(),
            config_type: Type::Bool,
            scope: Scope::UserEditable,
            default_value: Some(Value::Bool(false)),
            ..Field::default()
        }],
    };
    s.register(Package {
        key: "app.test@1".into(),
        id: "app.test".into(),
        schema: schema.clone(),
        base: Default::default(),
        product_locks: Default::default(),
    })
    .unwrap();
    s.register(Package {
        key: "app.test@2".into(),
        id: "app.test".into(),
        schema,
        base: Default::default(),
        product_locks: Default::default(),
    })
    .unwrap();
    s
}
#[test]
fn caller_identity_and_locked_user() {
    let mut s = service();
    assert!(s.authorize("app.other", 1, false, "app.test@1", 1).is_err());
    assert!(s.authorize("app.test", 1, false, "app.test@1", 2).is_err());
    assert!(s.authorize("app.test", 1, false, "app.test@1", 1).is_ok());
    s.lock_user(1);
    assert!(s.authorize("app.test", 1, true, "app.test@1", 1).is_err());
}
#[test]
fn rejects_spoofed_bindings() {
    let b = ServiceBinding::parse("prefs|PreferencesAdmin|Private|10,11||app.evil|0|fg").unwrap();
    assert!(Client::from_binding(1, &b).is_none());
    let b = ServiceBinding::parse(
        "bexos.preferences.UserPreferences|UserPreferences|Public|1,2,3||app.test|5|fg",
    )
    .unwrap();
    assert_eq!(Client::from_binding(1, &b).unwrap().uid, 5);
}
#[test]
fn compatible_versions_prepare_together() {
    let mut s = service();
    s.observers = vec![
        Observer {
            channel: 1,
            package: "app.test@1".into(),
            uid: 5,
            registered: true,
        },
        Observer {
            channel: 2,
            package: "app.test@2".into(),
            uid: 5,
            registered: true,
        },
        Observer {
            channel: 3,
            package: "app.test@2".into(),
            uid: 6,
            registered: true,
        },
    ];
    s.begin_user(
        "app.test@1",
        5,
        0,
        BTreeMap::from([("dark".into(), Value::Bool(true))]),
        false,
        0,
        5000,
    )
    .unwrap();
    let p = s.pending.as_mut().unwrap();
    assert_eq!(p.transaction.participants.len(), 2);
    p.transaction.prepare_reply(1, 1, true).unwrap();
    p.transaction.prepare_reply(2, 1, true).unwrap();
    assert_eq!(p.transaction.phase, Phase::Prepared);
    let candidate = s.candidate().unwrap();
    assert_eq!(
        candidate.effective("app.test@1", 5).unwrap(),
        candidate.effective("app.test@2", 5).unwrap()
    );
    assert_eq!(s.effective("app.test@1", 5).unwrap().0, 0);
}
#[test]
fn transplant_retains_identity_and_transactions() {
    let mut r = Runtime::empty();
    r.control = Channel(1);
    r.vfsd = Channel(2);
    r.usersd = Channel(3);
    r.service = service();
    r.service
        .begin_user("app.test@1", 5, 0, BTreeMap::new(), true, 0, 5000)
        .unwrap();
    let mut next = Runtime::empty();
    for key in r.keys() {
        let bytes = r.encode_record(key).unwrap().unwrap();
        next.adopt_record(key, Some(&bytes)).unwrap();
    }
    next.finish_adoption().unwrap();
    assert_eq!(next.service, r.service);
    assert_eq!(next.resources().len(), 3);
}

#[test]
fn reset_preserves_operator_and_product_policy() {
    let mut s = service();
    s.packages.get_mut("app.test@1").unwrap().schema.fields[0].scope = Scope::MdmLockable;
    s.packages.remove("app.test@2");
    s.begin_user(
        "app.test@1",
        5,
        0,
        [("dark".into(), Value::Bool(true))].into(),
        false,
        0,
        5000,
    )
    .unwrap();
    s = s.candidate().unwrap();
    s.pending = None;
    s.begin_operator(
        "app.test@1",
        0,
        BTreeMap::new(),
        ["dark".into()].into(),
        3,
        0,
        5000,
    )
    .unwrap();
    s = s.candidate().unwrap();
    s.pending = None;
    let (_, bytes) = s.effective("app.test@1", 5).unwrap();
    assert_eq!(
        bexos_component_config::ConfigTable::parse(&bytes)
            .unwrap()
            .get_bool("dark"),
        Ok(false)
    );
    let generation = s.effective("app.test@1", 5).unwrap().0;
    s.begin_user("app.test@1", 5, generation, BTreeMap::new(), true, 0, 5000)
        .unwrap();
    s = s.candidate().unwrap();
    s.pending = None;
    assert!(s.preferences.values().all(|p| p.values.is_empty()));
    assert!(s.operators["app.test@1"].locks.contains("dark"));
    s.packages
        .get_mut("app.test@1")
        .unwrap()
        .product_locks
        .insert("dark".into());
    assert_eq!(
        s.begin_operator(
            "app.test@1",
            1,
            BTreeMap::new(),
            ["dark".into()].into(),
            4,
            0,
            5000
        ),
        Err(Error::AccessDenied)
    );
}
#[test]
fn recovery_replays_all_operator_revisions_without_reusing_generations() {
    let mut s = service();
    s.ensure_user("app.test@1", 5).unwrap();
    let old = s.preferences.clone();
    for revision in 0..4 {
        s.begin_operator(
            "app.test@1",
            revision,
            [("dark".into(), Value::Bool(true))].into(),
            Default::default(),
            1,
            0,
            5000,
        )
        .unwrap();
        s = s.candidate().unwrap();
        s.pending = None;
    }
    let before = s.effective("app.test@1", 5).unwrap();
    s.preferences = old;
    assert!(s.ensure_user("app.test@1", 5).unwrap());
    assert_eq!(s.effective("app.test@1", 5).unwrap(), before);
    s.begin_user("app.test@1", 5, before.0, BTreeMap::new(), true, 0, 5000)
        .unwrap();
    assert!(s.candidate().unwrap().effective("app.test@1", 5).unwrap().0 > before.0);
}
#[test]
fn incompatible_versions_start_at_their_own_defaults() {
    let mut s = service();
    s.packages.get_mut("app.test@2").unwrap().schema.fields[0].scope = Scope::MdmLockable;
    s.begin_user(
        "app.test@1",
        5,
        0,
        [("dark".into(), Value::Bool(true))].into(),
        false,
        0,
        5000,
    )
    .unwrap();
    s = s.candidate().unwrap();
    s.pending = None;
    let (generation, bytes) = s.effective("app.test@2", 5).unwrap();
    assert_eq!(generation, 0);
    assert_eq!(
        bexos_component_config::ConfigTable::parse(&bytes)
            .unwrap()
            .get_bool("dark"),
        Ok(false)
    );
}
#[test]
fn transaction_holds_launches_and_other_writers() {
    let mut s = service();
    s.begin_user("app.test@1", 5, 0, BTreeMap::new(), true, 0, 5000)
        .unwrap();
    assert_eq!(s.ensure_user("app.test@2", 5), Err(Error::Busy));
    assert_eq!(
        s.begin_user("app.test@1", 5, 0, BTreeMap::new(), true, 0, 5000),
        Err(Error::Busy)
    );
    assert_eq!(
        s.register(s.packages["app.test@1"].clone()),
        Err(Error::Busy)
    );
}
#[test]
fn transplant_chunks_large_records_and_retains_authenticated_clients() {
    let mut r = Runtime::empty();
    r.control = Channel(1);
    r.vfsd = Channel(2);
    r.usersd = Channel(3);
    r.service = service();
    r.clients.push(Client {
        channel: 4,
        package: "app.test".into(),
        package_key: "app.test@1".into(),
        uid: 5,
        methods: vec![1, 2, 3],
        admin: false,
        manage: false,
        theme: false,
    });
    r.service.observers.push(Observer {
        channel: 5,
        package: "app.test@1".into(),
        uid: 5,
        registered: true,
    });
    r.service.theme_observers.push(ThemeObserver {
        channel: 6,
        uid: 5,
        generation: 9,
    });
    for p in r.service.packages.values_mut() {
        p.schema.fields.push(Field {
            name: "data".into(),
            config_type: Type::Bytes,
            scope: Scope::UserEditable,
            ..Field::default()
        });
    }
    let fp = r.service.packages["app.test@1"].schema.fingerprint();
    for uid in 1..200 {
        r.service.preferences.insert(
            (uid, "app.test".into(), fp),
            Preferences {
                generation: 3,
                values: [("data".into(), Value::Bytes(vec![42; 300]))].into(),
                revisions: Default::default(),
            },
        );
    }
    let mut next = Runtime::empty();
    assert!(r.keys().len() > 2);
    for key in r.keys() {
        let bytes = r.encode_record(key).unwrap().unwrap();
        assert!(bytes.len() <= bexos_userspace::live_migration::MAX_RECORD_DATA);
        next.adopt_record(key, Some(&bytes)).unwrap();
    }
    next.finish_adoption().unwrap();
    assert_eq!(next.clients, r.clients);
    assert_eq!(next.service, r.service);
    assert_eq!(next.resources().len(), 6);
}

#[test]
fn user_lock_clears_an_indeterminate_candidates_plaintext_without_aborting() {
    let mut runtime = Runtime::empty();
    runtime.service = service();
    runtime
        .service
        .begin_user(
            "app.test@1",
            5,
            0,
            [("dark".into(), Value::Bool(true))].into(),
            false,
            0,
            5000,
        )
        .unwrap();
    runtime.service = runtime.service.candidate().unwrap();
    runtime.service.pending.as_mut().unwrap().transaction.phase = Phase::Resolving;
    runtime.lock(5);
    assert!(runtime.service.preferences.is_empty());
    let pending = runtime.service.pending.as_ref().unwrap();
    assert_eq!(pending.transaction.phase, Phase::Resolving);
    match &pending.mutation {
        Mutation::User { values, .. } => assert!(values.is_empty()),
        _ => panic!("wrong mutation"),
    };
    assert_eq!(pending.response_generation, 1);
}

#[test]
fn aborted_attempts_never_reuse_transaction_ids() {
    let mut s = service();
    s.observers.push(Observer {
        channel: 1,
        package: "app.test@1".into(),
        uid: 5,
        registered: true,
    });
    s.begin_user(
        "app.test@1",
        5,
        0,
        [("dark".into(), Value::Bool(true))].into(),
        false,
        0,
        5000,
    )
    .unwrap();
    let first = s.pending.take().unwrap();
    s.begin_user("app.test@1", 5, 0, BTreeMap::new(), true, 0, 5000)
        .unwrap();
    let next = s.pending.as_mut().unwrap();
    assert!(next.transaction.generation > first.transaction.generation);
    assert_eq!(next.response_generation, first.response_generation);
    assert_eq!(
        next.transaction
            .prepare_reply(1, first.transaction.generation, true),
        Err(Error::Conflict)
    );
    assert_eq!(next.transaction.phase, Phase::Preparing);
}

#[test]
fn binding_retains_the_broker_selected_exact_version() {
    let metadata=ServiceBinding::parse("bexos.preferences.UserPreferences|UserPreferences|Public|1,2,3|caller-package-key:app.test:2.0.0|app.test|5|fg").unwrap();
    let client = Client::from_binding(1, &metadata).unwrap();
    assert_eq!(client.package_key, "app.test:2.0.0");
    assert_eq!(client.package, "app.test");
    assert_eq!(client.uid, 5);
}

#[test]
fn storage_package_names_cannot_escape_the_package_directory() {
    for id in [".", "..", "app/test", "app/../other"] {
        assert!(!valid_package(id));
    }
}

#[test]
fn shell_selectors_keep_system_policy_and_each_users_choice_through_migration() {
    let key = "bexos.platform.appd";
    let schema = Schema {
        fields: vec![
            Field {
                name: "sysui_package".into(),
                config_type: Type::String,
                max_size: 128,
                scope: Scope::SystemOnly,
                default_value: Some(Value::String("bexos.app.sysui".into())),
                ..Default::default()
            },
            Field {
                name: "userui_package".into(),
                config_type: Type::String,
                max_size: 128,
                scope: Scope::UserEditable,
                default_value: Some(Value::String("bexos.app.userui".into())),
                ..Default::default()
            },
        ],
    };
    let mut s = Service::default();
    s.register(Package {
        key: key.into(),
        id: key.into(),
        schema,
        base: Default::default(),
        product_locks: Default::default(),
    })
    .unwrap();
    let values =
        |field: &str, value: &str| BTreeMap::from([(field.into(), Value::String(value.into()))]);
    s.apply(&Mutation::Operator {
        package: key.into(),
        values: values("userui_package", "example.default"),
        locks: Default::default(),
    })
    .unwrap();
    s.apply(&Mutation::User {
        package: key.into(),
        uid: 1000,
        fingerprint: s.package(key).unwrap().schema.fingerprint(),
        values: values("userui_package", "example.alice"),
    })
    .unwrap();
    assert!(
        s.begin_user(
            key,
            1000,
            1,
            values("sysui_package", "evil.system"),
            false,
            0,
            5000
        )
        .is_err()
    );
    let mut source = Runtime::empty();
    source.control = Channel(1);
    source.vfsd = Channel(2);
    source.usersd = Channel(3);
    source.service = s;
    let mut restored = Runtime::empty();
    for key in source.keys() {
        restored
            .adopt_record(key, source.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    restored.finish_adoption().unwrap();
    for (uid, expected) in [(1000, "example.alice"), (1001, "example.default")] {
        let (_, bytes) = restored.service.effective(key, uid).unwrap();
        assert_eq!(
            bexos_component_config::ConfigTable::parse(&bytes)
                .unwrap()
                .get_string("userui_package")
                .unwrap(),
            expected
        );
    }
    restored.service.lock_user(1000);
    assert!(
        restored
            .service
            .authorize(key, 1000, false, key, 1000)
            .is_err()
    );
}
