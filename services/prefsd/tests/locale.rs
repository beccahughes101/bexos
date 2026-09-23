use bexos_component_config::{schema::*, transaction::Phase};
use bexos_prefsd::{binding::Client, locale, service::*};
use bexos_userspace::service_binding::ServiceBinding;
use std::collections::BTreeMap;
const ID: &str = "bexos.locale.preferences";
fn service() -> Service {
    let mut fields = Vec::new();
    for (name, value) in [
        ("language_priority", "en-US"),
        ("override_region", ""),
        ("measurement_system", "us"),
        ("hour_cycle", "h12"),
    ] {
        fields.push(Field {
            name: name.into(),
            config_type: Type::String,
            max_size: 263,
            scope: Scope::MdmLockable,
            default_value: Some(Value::String(value.into())),
            ..Field::default()
        });
    }
    fields.push(Field {
        name: "first_day_of_week".into(),
        config_type: Type::Uint32,
        scope: Scope::MdmLockable,
        default_value: Some(Value::Uint32(7)),
        ..Field::default()
    });
    let mut s = Service::default();
    s.register(Package {
        key: ID.into(),
        id: ID.into(),
        schema: Schema { fields },
        base: Default::default(),
        product_locks: Default::default(),
    })
    .unwrap();
    s
}
#[test]
fn language_validation_is_part_of_the_transaction() {
    for value in ["", "en_US", "en,en", "en,fr,de,it,es,ru,ar,ja,ko", "en--US"] {
        let mut s = service();
        assert!(
            s.begin_user(
                ID,
                7,
                0,
                BTreeMap::from([("language_priority".into(), Value::String(value.into()))]),
                false,
                0,
                5000
            )
            .is_err(),
            "{value}"
        );
        assert!(s.pending.is_none());
    }
    let mut s = service();
    s.begin_user(
        ID,
        7,
        0,
        BTreeMap::from([(
            "language_priority".into(),
            Value::String("es-mx, en-US".into()),
        )]),
        false,
        0,
        5000,
    )
    .unwrap();
    assert_eq!(locale::snapshot(&s, 7), Err(Error::Busy));
    let mut candidate = s.candidate().unwrap();
    candidate.pending.as_mut().unwrap().transaction.phase = Phase::Resolving;
    assert_eq!(locale::snapshot(&candidate, 7), Err(Error::Busy));
    candidate
        .pending
        .as_mut()
        .unwrap()
        .transaction
        .durable_commit()
        .unwrap();
    let (generation, settings) = locale::snapshot(&candidate, 7).unwrap();
    assert_eq!(generation, 1);
    assert_eq!(settings.languages, ["es-MX", "en-US"]);
    assert_eq!(settings.region, "es-MX");
    let (generation, settings) = locale::snapshot(&candidate, 8).unwrap();
    assert_eq!(generation, 0);
    assert_eq!(settings.languages, ["en-US"]);
    candidate.pending = None;
    candidate.lock_user(7);
    assert_eq!(locale::snapshot(&candidate, 7), Err(Error::AccessDenied));
}
#[test]
fn private_snapshot_rejects_other_packages_and_uids() {
    let binding = |caller, uid| {
        ServiceBinding::parse(&format!(
            "bexos.locale.LocalePreferences|LocalePreferences|LocaleRead|1,2||{caller}|{uid}|fg"
        ))
        .unwrap()
    };
    assert!(
        Client::from_binding(5, &binding("bexos.service.localed", 0))
            .unwrap()
            .locale
    );
    assert!(Client::from_binding(5, &binding("app.evil", 0)).is_none());
    assert!(Client::from_binding(5, &binding("bexos.service.localed", 7)).is_none());
}
#[test]
fn policy_locks_apply_to_locale_values() {
    let mut s = service();
    s.packages
        .get_mut(ID)
        .unwrap()
        .product_locks
        .insert("language_priority".into());
    assert!(
        s.begin_user(
            ID,
            7,
            0,
            BTreeMap::from([("language_priority".into(), Value::String("de-DE".into()))]),
            false,
            0,
            5000
        )
        .is_err()
    );
    assert_eq!(locale::snapshot(&s, 7).unwrap().1.languages, ["en-US"]);
}
