use bexos_flatland_input::{
    queue::EventQueue,
    router::Router,
    virtio::{Device, RawEvent},
    *,
};
#[test]
fn overflow_resets_instead_of_losing_release() {
    let mut q = EventQueue::<2>::default();
    for code in 1..=3 {
        q.push(Event::Key(Key {
            code,
            state: 1,
            ..Default::default()
        }));
    }
    assert_eq!(q.pop(), Some(Event::Reset));
    assert!(q.is_empty());
}
#[test]
fn keyboard_modifiers_and_repeat() {
    let mut d = Device::new(1);
    d.feed(
        RawEvent {
            kind: 1,
            code: 42,
            value: 1,
        },
        800.,
        600.,
    );
    let Some(Event::Key(k)) = d.feed(
        RawEvent {
            kind: 1,
            code: 30,
            value: 2,
        },
        800.,
        600.,
    ) else {
        panic!()
    };
    assert_eq!(k.unicode, 'A' as u32);
    assert_eq!(k.state, 2);
    assert_eq!(k.modifiers, 1);
}
#[test]
fn keyboard_has_one_recipient() {
    let mut r = Router::default();
    let key = Key {
        state: 1,
        ..Default::default()
    };
    assert!(r.key(key).is_none());
    r.focused = Some(7);
    assert_eq!(r.key(key).unwrap().view, 7);
    r.remove_view(7);
    assert!(r.key(key).is_none());
}

#[test]
fn focus_change_releases_old_recipient_and_suppresses_held_repeat() {
    let mut router = Router::default();
    router.focus(Some(7), |_| {});
    let press = Key {
        device: 1,
        code: 30,
        state: 1,
        unicode: 97,
        ..Default::default()
    };
    assert_eq!(router.key(press).unwrap().view, 7);
    let mut releases = Vec::new();
    router.focus(Some(8), |d| releases.push(d));
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].view, 7);
    assert!(matches!(
        releases[0].event,
        Event::Key(Key { state: 0, .. })
    ));
    assert!(router.key(Key { state: 2, ..press }).is_none());
    assert!(router.key(Key { state: 0, ..press }).is_none());
    assert_eq!(router.key(press).unwrap().view, 8);
}
#[test]
fn touch_and_repeat_state_survive_logical_migration() {
    use bexos_migration::codec::{Decoder, Encoder};
    let mut device = Device::new(9);
    let mut events = Vec::new();
    for (code, value) in [(0x2f, 2), (0x39, 19), (0x35, 16000), (0x36, 8000)] {
        device.feed_report(
            RawEvent {
                kind: 3,
                code,
                value,
            },
            800.,
            600.,
            0,
            |e| events.push(e),
        );
    }
    device.feed_report(
        RawEvent {
            kind: 0,
            code: 0,
            value: 0,
        },
        800.,
        600.,
        0,
        |e| events.push(e),
    );
    assert!(matches!(
        events[0],
        Event::Pointer(Pointer {
            id: 3,
            phase: Phase::Down,
            ..
        })
    ));
    device.feed_at(
        RawEvent {
            kind: 1,
            code: 30,
            value: 1,
        },
        800.,
        600.,
        10,
    );
    let mut w = Encoder::new();
    device.encode(&mut w);
    let bytes = w.finish();
    let mut r = Decoder::new(&bytes);
    let mut restored = Device::decode(&mut r).unwrap();
    r.finish().unwrap();
    assert!(restored.repeat(500009, 800., 600.).is_none());
    assert!(matches!(
        restored.repeat(500010, 800., 600.),
        Some(Event::Key(Key {
            state: 2,
            code: 30,
            ..
        }))
    ));
    events.clear();
    restored.feed_report(
        RawEvent {
            kind: 3,
            code: 0x39,
            value: -1,
        },
        800.,
        600.,
        0,
        |e| events.push(e),
    );
    restored.feed_report(
        RawEvent {
            kind: 0,
            code: 0,
            value: 0,
        },
        800.,
        600.,
        0,
        |e| events.push(e),
    );
    assert!(matches!(
        events[0],
        Event::Pointer(Pointer {
            id: 3,
            phase: Phase::Up,
            ..
        })
    ));
}
#[test]
fn tablet_coordinates_use_device_range() {
    let mut d = Device::new(1);
    d.axes = [-100, 100, 0, 1000];
    d.feed(
        RawEvent {
            kind: 3,
            code: 0,
            value: 0,
        },
        801.,
        601.,
    );
    d.feed(
        RawEvent {
            kind: 3,
            code: 1,
            value: 500,
        },
        801.,
        601.,
    );
    assert!(matches!(
        d.feed(
            RawEvent {
                kind: 0,
                code: 0,
                value: 0
            },
            801.,
            601.
        ),
        Some(Event::Pointer(Pointer {
            x: 400.,
            y: 300.,
            ..
        }))
    ));
}

#[test]
fn reused_touch_slot_cancels_before_delivering_new_contact() {
    let mut device = Device::new(7);
    let mut events = Vec::new();
    for tracking in [10, 11] {
        device.feed_report(
            RawEvent {
                kind: 3,
                code: 0x39,
                value: tracking,
            },
            800.,
            600.,
            0,
            |e| events.push(e),
        );
        device.feed_report(RawEvent::default(), 800., 600., 0, |e| events.push(e));
    }
    let phases: Vec<_> = events
        .iter()
        .map(|e| match e {
            Event::Pointer(p) => p.phase,
            _ => panic!(),
        })
        .collect();
    assert_eq!(phases, vec![Phase::Down, Phase::Cancel, Phase::Down]);
}

#[test]
fn committed_removal_cancels_idle_contacts_and_releases_focus() {
    let mut router = Router::default();
    let pointer = Pointer {
        device: 7,
        id: 3,
        phase: Phase::Down,
        ..Default::default()
    };
    router.captures.insert(
        (7, 3),
        bexos_flatland_input::router::Capture {
            view: 4,
            node: 1,
            last: pointer,
        },
    );
    router.focus(Some(4), |_| {});
    let key = Key {
        device: 7,
        code: 30,
        state: 1,
        ..Default::default()
    };
    router.key(key).unwrap();
    let mut releases = Vec::new();
    router.reconcile(&Default::default(), |delivery| releases.push(delivery));
    assert_eq!(releases.len(), 2);
    assert!(releases.iter().all(|delivery| delivery.view == 4));
    assert!(matches!(
        releases[0].event,
        Event::Pointer(Pointer {
            phase: Phase::Cancel,
            ..
        })
    ));
    assert!(matches!(
        releases[1].event,
        Event::Key(Key { state: 0, .. })
    ));
    assert_eq!(router.focused, None);
    assert!(router.captures.is_empty());
    router.focus(Some(9), |_| {});
    assert!(router.key(Key { state: 2, ..key }).is_none());
}
