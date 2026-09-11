use bexos_component_config::{
    schema::*,
    storage::{self, Files},
    transaction::{Phase, Receiver, Transaction},
    *,
};
use std::collections::{BTreeMap, BTreeSet};
fn schema() -> Schema {
    Schema {
        fields: vec![
            Field {
                name: "dark".into(),
                config_type: Type::Bool,
                default_value: Some(Value::Bool(false)),
                scope: Scope::UserEditable,
                ..Field::default()
            },
            Field {
                name: "cache".into(),
                config_type: Type::Uint32,
                default_value: Some(Value::Uint32(64)),
                scope: Scope::MdmLockable,
                constraint: Some(Constraint::UintRange { min: 32, max: 1024 }),
                ..Field::default()
            },
            Field {
                name: "secret".into(),
                config_type: Type::String,
                default_value: Some(Value::String("base".into())),
                max_size: 8,
                ..Field::default()
            },
        ],
    }
}
fn assignments(name: &str, value: Value) -> Assignments {
    [(name.into(), value)].into()
}
#[test]
fn schema_roundtrip_constraints_and_default_scope() {
    let s = schema();
    assert_eq!(Schema::decode(&s.encode()).unwrap(), s);
    assert!(s.validate().is_ok());
    assert!(
        s.validate_assignments(
            &assignments("cache", Value::Uint32(31)),
            true,
            &BTreeSet::new()
        )
        .is_err()
    );
    assert_eq!(
        s.validate_assignments(
            &assignments("secret", Value::String("user".into())),
            true,
            &BTreeSet::new()
        ),
        Err(Error::AccessDenied)
    );
}
#[test]
fn layering_retains_locked_values() {
    let s = schema();
    let user = assignments("cache", Value::Uint32(256));
    let op = assignments("dark", Value::Bool(true));
    let locks = BTreeSet::from(["cache".into()]);
    let effective = s.resolve(&Assignments::new(), &op, &user, &locks).unwrap();
    assert_eq!(effective["cache"], Value::Uint32(64));
    assert_eq!(effective["dark"], Value::Bool(true));
    assert_eq!(
        s.resolve(&Assignments::new(), &op, &user, &BTreeSet::new())
            .unwrap()["cache"],
        Value::Uint32(256)
    );
    assert_eq!(
        s.validate_assignments(&user, true, &locks),
        Err(Error::AccessDenied)
    );
}
#[test]
fn fingerprint_preserves_legacy_and_tracks_constraints() {
    let mut s = schema();
    for f in &mut s.fields {
        f.scope = Scope::SystemOnly;
        f.constraint = None;
    }
    let old = schema_fingerprint(
        &s.fields
            .iter()
            .map(|f| {
                (
                    f.name.as_str(),
                    f.config_type.scalar().unwrap(),
                    f.required,
                    f.max_size,
                )
            })
            .collect::<Vec<_>>(),
    );
    assert_eq!(s.fingerprint(), old);
    s.fields[0].scope = Scope::UserEditable;
    assert_ne!(s.fingerprint(), old);
    let fp = s.fingerprint();
    s.fields[0].display_name = "Theme".into();
    assert_eq!(fp, s.fingerprint());
}
#[test]
fn rejects_duplicate_bad_scalar_and_fingerprint() {
    assert!(
        ConfigTable::parse(&encode_config(&[
            ("x", ConfigType::Bool, &[0]),
            ("x", ConfigType::Bool, &[1])
        ]))
        .is_err()
    );
    assert!(
        ConfigTable::parse(&encode_config(&[(
            "x",
            ConfigType::Uint32,
            &[1, 2, 3, 4, 5]
        )]))
        .is_err()
    );
    let s = schema();
    let bytes = encode_config_v2(4, 1, &[]).unwrap();
    assert_eq!(s.decode_table(&bytes), Err(Error::Fingerprint));
}
#[test]
fn all_receivers_prepare_before_commit() {
    let mut tx = Transaction::new(0, 0, 0, 5000, [1, 2].into()).unwrap();
    assert_eq!(tx.durable_commit(), Err(Error::Busy));
    tx.prepare_reply(1, 1, true).unwrap();
    tx.prepare_reply(2, 1, true).unwrap();
    tx.durable_commit().unwrap();
    assert_eq!(tx.phase, Phase::Committed);
    assert_eq!(tx.abort(), Err(Error::Conflict));
    tx.commit_reply(1, 1).unwrap();
    tx.commit_reply(2, 1).unwrap();
    assert_eq!(tx.phase, Phase::Complete);
}
#[test]
fn reject_disconnect_timeout_and_conflicts() {
    assert_eq!(
        Transaction::new(1, 0, 0, 5000, BTreeSet::new()),
        Err(Error::Conflict)
    );
    for failure in 0..3 {
        let mut tx = Transaction::new(0, 0, 0, 5000, [1].into()).unwrap();
        assert!(
            match failure {
                0 => tx.prepare_reply(1, 1, false),
                1 => tx.disconnect(1),
                _ => tx.expire(5000),
            }
            .is_err()
        );
        assert_eq!(tx.phase, Phase::Aborted);
        assert!(tx.durable_commit().is_err());
    }
}
#[test]
fn receiver_keeps_snapshots_and_commit_is_idempotent() {
    let mut r = Receiver::new(5, 0);
    let old = r.snapshot();
    r.prepare(1, 6, |_| true).unwrap();
    assert_eq!(*r.snapshot(), 5);
    r.commit(1, |v| assert_eq!(*v, 6)).unwrap();
    r.commit(1, |_| panic!("duplicate callback")).unwrap();
    assert_eq!(*old, 5);
    assert_eq!(*r.snapshot(), 6);
    r.prepare(2, 7, |_| true).unwrap();
    r.abort(2);
    assert!(r.commit(2, |_| {}).is_err());
}
#[derive(Default)]
struct MemoryFiles {
    files: BTreeMap<String, Vec<u8>>,
    fail: Option<String>,
}
impl Files for MemoryFiles {
    fn read(&mut self, n: &str) -> Result<Option<Vec<u8>>, Error> {
        Ok(self.files.get(n).cloned())
    }
    fn write_sync(&mut self, n: &str, b: &[u8]) -> Result<(), Error> {
        if self.fail.as_deref() == Some(n) {
            self.files.insert(n.into(), b[..b.len() / 2].to_vec());
            return Err(Error::Storage);
        }
        self.files.insert(n.into(), b.to_vec());
        Ok(())
    }
}
#[test]
fn interrupted_storage_keeps_last_committed_snapshot() {
    for fail in ["1.bexpref", "1.commit"] {
        let mut f = MemoryFiles::default();
        let a = encode_config_v2(1, 1, &[]).unwrap();
        storage::commit(&mut f, 0, &a).unwrap();
        f.fail = Some(fail.into());
        let b = encode_config_v2(1, 2, &[]).unwrap();
        assert!(storage::commit(&mut f, 1, &b).is_err());
        assert_eq!(storage::load(&mut f).unwrap().unwrap().bytes, a);
    }
}
#[test]
fn reboot_storage_and_stale_write() {
    let mut f = MemoryFiles::default();
    for generation in 1..5 {
        let b = encode_config_v2(1, generation, &[]).unwrap();
        storage::commit(&mut f, generation - 1, &b).unwrap();
        assert_eq!(
            storage::load(&mut f).unwrap().unwrap().generation,
            generation
        );
    }
    assert_eq!(
        storage::commit(&mut f, 1, &encode_config_v2(1, 2, &[]).unwrap()),
        Err(Error::Conflict)
    );
}

#[test]
fn uncertain_commit_is_replayed_and_cannot_abort() {
    let mut files = MemoryFiles::default();
    let first = encode_config_v2(1, 1, &[]).unwrap();
    storage::commit(&mut files, 0, &first).unwrap();
    let second = encode_config_v2(1, 2, &[]).unwrap();
    files.fail = Some("1.commit".into());
    assert_eq!(
        storage::commit(&mut files, 1, &second),
        Err(Error::CommitUncertain)
    );
    let mut tx = Transaction::new(1, 1, 0, 5000, BTreeSet::new()).unwrap();
    tx.phase = Phase::Resolving;
    assert_eq!(tx.abort(), Err(Error::Conflict));
    files.fail = None;
    storage::commit(&mut files, 1, &second).unwrap();
    storage::commit(&mut files, 1, &second).unwrap();
    tx.durable_commit().unwrap();
    assert_eq!(storage::load(&mut files).unwrap().unwrap().generation, 2);
}
#[test]
fn committed_receiver_disconnect_releases_delivery() {
    let mut tx = Transaction::new(0, 0, 0, 5000, [1, 2].into()).unwrap();
    tx.prepare_reply(1, 1, true).unwrap();
    tx.prepare_reply(2, 1, true).unwrap();
    tx.durable_commit().unwrap();
    tx.disconnect(1).unwrap();
    tx.commit_reply(2, 1).unwrap();
    assert_eq!(tx.phase, Phase::Complete);
}
#[test]
fn string_enums_and_unsigned_limits_validate_defaults_and_overlays() {
    let mut s = Schema {
        fields: vec![Field {
            name: "theme".into(),
            config_type: Type::String,
            scope: Scope::UserEditable,
            max_size: 5,
            constraint: Some(Constraint::StringEnum(vec!["light".into(), "dark".into()])),
            default_value: Some(Value::String("light".into())),
            ..Field::default()
        }],
    };
    assert!(s.validate().is_ok());
    assert!(
        s.decode_table(&encode_config(&[("theme", ConfigType::String, b"other")]))
            .is_err()
    );
    s.fields[0].default_value = Some(Value::String("other".into()));
    assert!(s.validate().is_err());
    s.fields[0].default_value = Some(Value::String("dark".into()));
    let fp = s.fingerprint();
    s.fields[0].constraint = Some(Constraint::StringEnum(vec!["dark".into()]));
    assert_ne!(fp, s.fingerprint());
    let mut s = schema();
    s.fields[1].constraint = Some(Constraint::UintRange {
        min: 0,
        max: u64::MAX,
    });
    assert!(s.validate().is_err());
}
#[test]
fn malformed_committed_storage_is_an_error_without_fallback() {
    let mut files = MemoryFiles::default();
    files.files.insert("0.commit".into(), vec![0; 4]);
    assert_eq!(storage::load(&mut files), Err(Error::Storage));
    for bytes in [
        encode_config(&[("flag", ConfigType::Bool, &[2])]),
        encode_config(&[("s", ConfigType::String, &[255])]),
    ] {
        assert!(ConfigTable::parse(&bytes).is_err());
    }
}
