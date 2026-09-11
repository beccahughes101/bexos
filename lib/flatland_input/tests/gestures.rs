use bexos_flatland_input::{Phase, Pointer, gestures::*};
use bexos_migration::codec::{Decoder, Encoder};
fn pointer(x: f64, y: f64, phase: Phase) -> Pointer {
    Pointer {
        device: 1,
        id: 3,
        x,
        y,
        phase,
        ..Default::default()
    }
}
#[test]
fn edge_takeover_cancels_only_after_threshold_and_preserves_progress() {
    let mut g = Gestures::default();
    g.configure(
        EdgePolicy {
            edges: 1,
            ..Default::default()
        },
        |_| panic!(),
    )
    .unwrap();
    let down = pointer(2., 30., Phase::Down);
    assert_eq!(g.pointer(down, 800., 600.), Decision::Pass);
    assert_eq!(
        g.pointer(pointer(20., 31., Phase::Move), 800., 600.),
        Decision::Pass
    );
    let mut w = Encoder::new();
    g.encode(&mut w);
    let bytes = w.finish();
    let mut r = Decoder::new(&bytes);
    let mut g = Gestures::decode(&mut r).unwrap();
    r.finish().unwrap();
    let moved = pointer(40., 31., Phase::Move);
    assert_eq!(
        g.pointer(moved, 800., 600.),
        Decision::Claim {
            start: down,
            event: moved
        }
    );
    let mut w = Encoder::new();
    g.encode(&mut w);
    let bytes = w.finish();
    let mut r = Decoder::new(&bytes);
    let mut g = Gestures::decode(&mut r).unwrap();
    let up = pointer(50., 31., Phase::Up);
    assert_eq!(g.pointer(up, 800., 600.), Decision::Owned(up));
    assert_eq!(
        g.pointer(pointer(55., 31., Phase::Move), 800., 600.),
        Decision::Pass
    );
}
#[test]
fn cross_axis_motion_relinquishes_candidate_and_device_loss_cancels_claims() {
    let mut g = Gestures::default();
    g.configure(
        EdgePolicy {
            edges: 15,
            ..Default::default()
        },
        |_| {},
    )
    .unwrap();
    g.pointer(pointer(2., 30., Phase::Down), 800., 600.);
    assert_eq!(
        g.pointer(pointer(3., 80., Phase::Move), 800., 600.),
        Decision::Pass
    );
    assert_eq!(
        g.pointer(pointer(80., 80., Phase::Move), 800., 600.),
        Decision::Pass
    );
    g.pointer(pointer(799., 30., Phase::Down), 800., 600.);
    assert!(matches!(
        g.pointer(pointer(700., 30., Phase::Move), 800., 600.),
        Decision::Claim { .. }
    ));
    let mut canceled = Vec::new();
    g.remove_device(1, |p| canceled.push(p));
    assert_eq!(canceled.len(), 1);
    assert_eq!(canceled[0].phase, Phase::Cancel);
    assert_eq!(
        g.pointer(pointer(690., 30., Phase::Move), 800., 600.),
        Decision::Pass
    );
}
