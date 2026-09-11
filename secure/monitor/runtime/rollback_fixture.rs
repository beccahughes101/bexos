//! Test-only persistent-floor provisioning. This module is never linked into
//! product monitors. A subsequent, ordinary product boot must reject its older
//! signed image using the actual authenticated RPMB state left by this guest.
pub fn provision_journal(transport: &mut impl bexos_trusty_boot::ql::Transport) {
    use bexos_trusty_boot::journal;
    let mut control = [0; 64];
    let length = match unsafe {
        bexos_secure_monitor::fw_cfg::read_bounded(b"opt/bexos/fixture-floor", &mut control)
    } {
        Err(bexos_secure_monitor::fw_cfg::FileError::NotFound) => return,
        Ok(length) => length,
        Err(_) => panic!("invalid rollback fixture file"),
    };
    let component = match &control[..length] {
        b"journal_component: 1\n" => 1,
        b"journal_component: 2\n" => 2,
        _ => return, // The AVB fixture below validates its separate configuration.
    };
    let old = journal::query(transport, 2, component).expect("authenticated journal query");
    // This signed negative fixture intentionally records an unavailable future
    // image. It tests durable reboot rejection, never claims image activation.
    let committed = journal::Record {
        architecture: 2,
        component,
        slot: 3 - old.slot,
        generation: old.generation.checked_add(1).unwrap(),
        previous: old.generation,
        image_hash: [0x77; 32],
    };
    assert_eq!(journal::commit(transport, committed), Ok(committed));
    // New kernel IPC and TP storage sessions read the committed bytes again.
    assert_eq!(journal::query(transport, 2, component), Ok(committed));
    assert_eq!(journal::commit(transport, committed), Ok(committed));
    let conflict = journal::Record {
        image_hash: [0x78; 32],
        ..committed
    };
    assert_eq!(
        journal::commit(transport, conflict),
        Err(journal::Error::Uncertain)
    );
    assert_eq!(journal::query(transport, 2, component), Ok(committed));
    crate::log(if component == 1 {
        "monitor-fixture: Trusty replacement journal durably committed\n"
    } else {
        "monitor-fixture: monitor replacement journal durably committed\n"
    });
    crate::halt();
}

pub fn provision<T: bexos_trusty_boot::ql::Transport>(
    avb: &mut bexos_trusty_boot::avb::Avb<T>,
    verified: bexos_secure_monitor::boot_verify::Verified,
) -> ! {
    let mut control = [0; 64];
    let location = match unsafe {
        bexos_secure_monitor::fw_cfg::read_bounded(b"opt/bexos/fixture-floor", &mut control)
    } {
        Err(bexos_secure_monitor::fw_cfg::FileError::NotFound) => verified.rollback_location,
        Ok(length) => match &control[..length] {
            b"rollback_location: 30\n" => 30,
            b"rollback_location: 31\n" => 31,
            _ => panic!("invalid rollback fixture configuration"),
        },
        Err(_) => panic!("invalid rollback fixture file"),
    };
    let old = avb.read_rollback(location).unwrap();
    let generation = verified.generation.max(old).checked_add(1).unwrap();
    avb.write_rollback(location, generation).unwrap();
    assert_eq!(avb.read_rollback(location), Ok(generation));
    avb.close().unwrap();
    crate::log("monitor-fixture: authenticated RPMB floor provisioned\n");
    match location {
        30 => crate::log("monitor-fixture: Trusty generation floor provisioned\n"),
        31 => crate::log("monitor-fixture: monitor generation floor provisioned\n"),
        _ => {}
    }
    crate::halt();
}
