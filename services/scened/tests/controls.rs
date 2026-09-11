use bexos_graphics::{resolved::Rect, scene::Session};
use bexos_graphics_runtime::migration::Runtime;
use bexos_scened::{
    controls::{Controls, FocusRing, View},
    state::Scene,
};
use bexos_userspace::{Channel, live_migration::State, service_binding::BoundServiceEndpoint};
#[test]
fn control_capabilities_and_logical_effects_survive_global_deltas() {
    let mut s = Scene::default();
    s.sessions.insert(10, Session::default());
    s.controls.views.insert(
        10,
        View {
            token: 51,
            identity: 99,
            generation: 7,
        },
    );
    s.controls.committed.insert(10, 7);
    s.controls.register(10);
    s.controls.generation = 12;
    s.controls.ring = Some(FocusRing {
        view: 10,
        generation: 7,
        bounds: Rect {
            x: 1.,
            y: 2.,
            width: 30.,
            height: 40.,
        },
        width: 2,
        rgba: 0xffa000ff,
    });
    s.controls
        .accessibility
        .push(BoundServiceEndpoint::new(Channel(60), vec![1, 2, 3, 4]));
    assert_eq!(s.controls.lookup_identity(99).unwrap(), 10);
    assert!(s.controls.lookup_identity(98).is_err());
    let bytes = s.controls.encode();
    let restored = Controls::decode(&bytes).unwrap();
    assert_eq!(restored.views[&10].token, 51);
    for n in 0..bytes.len() {
        assert!(Controls::decode(&bytes[..n]).is_err());
    }
    let source = Runtime::new(Channel(1), Some(Channel(2)), s);
    let mut target = Runtime::<Scene>::empty();
    for key in source.keys() {
        target
            .adopt_record(key, source.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    target
        .adopt_record(1, source.encode_record(1).unwrap().as_deref())
        .unwrap();
    target.finish_adoption().unwrap();
    assert_eq!(target.component.controls.lookup_identity(99).unwrap(), 10);
    assert_eq!(target.component.controls.ring.unwrap().generation, 7);
    assert_eq!(target.component.controls.accessibility[0].channel.0, 60);
    assert_eq!(target.component.controls.generation, 12);
}
#[test]
fn duplicate_capability_identities_and_invalid_display_transforms_are_rejected() {
    let mut c = Controls::default();
    c.views.insert(
        1,
        View {
            token: 9,
            identity: 33,
            generation: 0,
        },
    );
    c.views.insert(
        2,
        View {
            token: 10,
            identity: 33,
            generation: 0,
        },
    );
    assert!(c.validate().is_err());
    c.views.remove(&2);
    c.stacking = vec![1, 1];
    assert!(c.validate().is_err());
    c.stacking = vec![1];
    c.display.scale = 0.;
    assert!(c.validate().is_err());
    c.display.scale = 1.;
    c.validate().unwrap();
}
#[test]
fn pending_visual_controls_are_isolated_until_the_frame_boundary_and_migrate() {
    use bexos_graphics::accessibility::DisplayTransform;
    let mut c = Controls::default();
    c.register(1);
    c.register(2);
    c.pending_display = Some(DisplayTransform {
        scale: 2.,
        origin_x: 10.,
        origin_y: 5.,
        filter: 1,
    });
    c.pending_stacking = Some(vec![2, 1]);
    assert_eq!(c.display.scale, 1.);
    assert_eq!(c.stacking, [1, 2]);
    let mut restored = Controls::decode(&c.encode()).unwrap();
    assert_eq!(restored.display.scale, 1.);
    assert_eq!(restored.generation, 0);
    assert!(restored.latch());
    assert_eq!(restored.display.scale, 2.);
    assert_eq!(restored.stacking, [2, 1]);
    assert_eq!(restored.generation, 1);
    assert!(!restored.latch());
}
#[test]
fn embedded_view_links_and_feedback_survive_chunked_transfer() {
    let mut s = Scene::default();
    let mut parent = Session::default();
    parent.pending.create(1).unwrap();
    parent.pending.root = Some(1);
    parent.pending.node(1).unwrap().embedded = Some(20);
    parent.committed = parent.pending.clone();
    s.sessions.insert(10, parent);
    s.sessions.insert(20, Session::default());
    s.controls.embedded.insert(20);
    s.controls.register(10);
    s.controls.register(20);
    let queue = s.queues.entry(10).or_default();
    queue.sequence = 7;
    queue.latched_sequence = 7;
    queue.presented_sequence = 6;
    queue.rejected_sequence = 5;
    queue.presented_at = 400;
    let source = Runtime::new(Channel(1), Some(Channel(2)), s);
    let mut target = Runtime::<Scene>::empty();
    for key in source.keys() {
        target
            .adopt_record(key, source.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    target.finish_adoption().unwrap();
    assert_eq!(
        target.component.sessions[&10].committed.nodes[&1].embedded,
        Some(20)
    );
    assert!(target.component.controls.embedded.contains(&20));
    let queue = &target.component.queues[&10];
    assert_eq!(
        (
            queue.latched_sequence,
            queue.presented_sequence,
            queue.rejected_sequence,
            queue.presented_at
        ),
        (7, 6, 5, 400)
    );
}
