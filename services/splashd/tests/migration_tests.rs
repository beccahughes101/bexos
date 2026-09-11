use bexos_graphics::{Format, Surface, progress::Progress};
use bexos_graphics_runtime::{Mapping, canvas::Canvas, migration::Runtime};
use bexos_splashd::state::Splash;
use bexos_userspace::{Channel, live_migration::State, service_binding::BoundServiceEndpoint};
fn source() -> Runtime<Splash> {
    let canvas = Canvas {
        display: Channel(4),
        surface: Surface {
            width: 8,
            height: 8,
            stride: 32,
            format: Format::Bgra,
        },
        output: Mapping {
            handle: 5,
            address: 4096,
            size: 256,
            rights: 6,
            owned: false,
        },
        generation: 1,
        client_id: 9,
    };
    let mut r = Runtime::new(
        Channel(1),
        Some(Channel(2)),
        Splash {
            canvas: Some(canvas),
            progress: Progress {
                stage: 4,
                percent: 90,
                message: "Ready".into(),
            },
            ack: Some(Channel(8)),
            deadline: 1_000_000,
            handoff_generation: 2,
            frozen: true,
            frames: 45,
            ..Default::default()
        },
    );
    r.clients
        .push(BoundServiceEndpoint::new(Channel(3), vec![1, 2]));
    r
}
#[test]
fn transplant_preserves_pending_handoff_and_rights() {
    let old = source();
    let mut new = Runtime::<Splash>::empty();
    for key in old.keys() {
        new.adopt_record(key, old.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    new.validate().unwrap();
    assert_eq!(new.component.progress, old.component.progress);
    assert!(new.component.frozen);
    assert_eq!(new.component.handoff_generation, 2);
    assert_eq!(new.component.frames, 45);
    assert_eq!(new.clients[0].allowed_methods, vec![1, 2]);
    assert!(!new.component.canvas.unwrap().output.owned);
}
#[test]
fn rejected_candidate_keeps_source_state() {
    let old = source();
    let before = old.encode_record(1).unwrap();
    let mut new = Runtime::<Splash>::empty();
    assert!(new.adopt_record(1, Some(&[0; 8])).is_err());
    assert_eq!(before, old.encode_record(1).unwrap());
    assert!(old.component.frozen);
}
