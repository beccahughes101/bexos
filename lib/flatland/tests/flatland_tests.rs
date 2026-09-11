use bexos_flatland::{presentation::Queue, resolved::Snapshot, scene::Graph, *};
fn surface(w: u32, h: u32) -> Surface {
    Surface {
        width: w,
        height: h,
        stride: w * 4,
        format: Format::Bgra,
    }
}
fn graph() -> Graph {
    let mut g = Graph::default();
    g.create(1).unwrap();
    g.root = Some(1);
    g.node(1).unwrap().content = Some((7, surface(4, 4)));
    g
}
#[test]
fn geometry_is_shared_by_composition_and_hit_testing() {
    let mut g = graph();
    let node = g.node(1).unwrap();
    node.translation = (3, 2);
    node.scale = (2., 2.);
    node.clip = Some((2, 2));
    let mut s = Snapshot::default();
    s.compile(&g, surface(16, 16)).unwrap();
    assert_eq!(s.hit_test(5., 4.), Some((1, 1., 1.)));
    assert_eq!(s.hit_test(7., 4.), None);
    let mut out = vec![0; 16 * 16 * 4];
    let pixels = [255; 64];
    composition::composite(
        &s,
        surface(16, 16),
        &mut out,
        surface(16, 16).full(),
        |_| Some(&pixels[..]),
    )
    .unwrap();
    assert_eq!(&out[(4 * 16 + 5) * 4..(4 * 16 + 5) * 4 + 4], &[255; 4]);
    assert_eq!(&out[(4 * 16 + 7) * 4..(4 * 16 + 7) * 4 + 4], &[0; 4]);
}
#[test]
fn damage_does_not_touch_other_pixels_or_request_invisible_buffers() {
    let mut s = Snapshot::default();
    s.compile(&graph(), surface(16, 16)).unwrap();
    let mut out = vec![19; 16 * 16 * 4];
    composition::composite::<&[u8]>(
        &s,
        surface(16, 16),
        &mut out,
        Damage {
            x: 8,
            y: 8,
            width: 2,
            height: 2,
        },
        |_| panic!("unaffected surface"),
    )
    .unwrap();
    assert!(out.iter().all(|p| *p == 19));
}
#[test]
fn presentations_are_bounded_atomic_and_ordered() {
    let mut g = graph();
    let mut q = Queue::default();
    q.submit(&g, 20).unwrap();
    g.node(1).unwrap().translation = (10, 0);
    q.submit(&g, 30).unwrap();
    q.submit(&g, 40).unwrap();
    assert_eq!(q.submit(&g, 50), Err(Error::Busy));
    assert!(q.latch(19).is_none());
    assert_eq!(q.latch(20).unwrap().graph.nodes[&1].translation, (0, 0));
    assert_eq!(q.submit(&g, 25), Err(Error::Stale));
    q.validate().unwrap();
}
#[test]
fn failed_compile_preserves_previous_snapshot() {
    let mut s = Snapshot::default();
    s.compile(&graph(), surface(16, 16)).unwrap();
    let mut broken = graph();
    broken.nodes.get_mut(&1).unwrap().scale = (f32::NAN, 1.);
    assert!(s.compile(&broken, surface(16, 16)).is_err());
    assert_eq!(s.hit_test(0., 0.), Some((1, 0., 0.)));
}

#[test]
fn geometry_cache_recomputes_changed_subtrees_and_retains_paint_order() {
    let mut g = graph();
    g.node(1).unwrap().content = None;
    for id in 2..=5 {
        g.create(id).unwrap();
        g.node(id).unwrap().content = Some((id, surface(4, 4)));
    }
    g.attach(1, 2).unwrap();
    g.attach(1, 3).unwrap();
    g.attach(2, 4).unwrap();
    g.attach(3, 5).unwrap();
    let mut s = Snapshot::default();
    s.compile(&g, surface(100, 100)).unwrap();
    assert_eq!(s.recomputed_nodes(), 5);
    s.compile(&g, surface(100, 100)).unwrap();
    assert_eq!(s.recomputed_nodes(), 0);
    g.node(2).unwrap().translation = (10, 0);
    s.compile(&g, surface(100, 100)).unwrap();
    assert_eq!(s.recomputed_nodes(), 2);
    assert_eq!(s.hit_test(10., 0.), Some((4, 0., 0.)));
    assert_eq!(s.hit_test(0., 0.), Some((5, 0., 0.)));
    g.reorder(1, 2, 1).unwrap();
    s.compile(&g, surface(100, 100)).unwrap();
    assert_eq!(s.recomputed_nodes(), 0);
    assert_eq!(
        s.items.iter().map(|i| i.node).collect::<Vec<_>>(),
        [3, 5, 2, 4]
    );
    g.node(2).unwrap().opacity = 0.;
    s.compile(&g, surface(100, 100)).unwrap();
    assert_eq!(s.hit_test(10., 0.), None);
    g.node(2).unwrap().opacity = 1.;
    g.detach(2, 4).unwrap();
    g.attach(3, 4).unwrap();
    s.compile(&g, surface(100, 100)).unwrap();
    assert_eq!(s.hit_test(0., 0.), Some((4, 0., 0.)));
}

#[test]
fn rejected_cached_update_does_not_publish_partial_geometry() {
    let mut g = graph();
    g.create(2).unwrap();
    g.attach(1, 2).unwrap();
    g.node(2).unwrap().content = Some((8, surface(2, 2)));
    let mut s = Snapshot::default();
    s.compile(&g, surface(100, 100)).unwrap();
    let previous = s.items.clone();
    g.node(1).unwrap().translation = (10, 0);
    g.node(2).unwrap().content.as_mut().unwrap().1.stride = 1;
    assert!(s.compile(&g, surface(100, 100)).is_err());
    assert_eq!(s.items, previous);
    g.node(2).unwrap().content.as_mut().unwrap().1.stride = 8;
    s.compile(&g, surface(100, 100)).unwrap();
    assert_eq!(s.recomputed_nodes(), 2);
    assert_eq!(s.hit_test(10., 0.), Some((2, 0., 0.)));
}

#[test]
fn detached_cycles_and_noninvertible_geometry_are_rejected() {
    let mut g = graph();
    g.create(2).unwrap();
    g.create(3).unwrap();
    g.node(2).unwrap().children.push(3);
    g.node(3).unwrap().children.push(2);
    assert!(g.validate().is_err());
    g.node(3).unwrap().children.clear();
    g.validate().unwrap();
    for id in 4..=28 {
        g.create(id).unwrap();
        g.node(id).unwrap().scale = (1e-14, 1.);
        g.node(id - 1).unwrap().children.push(id);
    }
    assert!(g.validate().is_err());
}

#[test]
fn acquire_gates_latching_and_release_waits_for_retirement() {
    use synchronization::{Stage, Synchronization};
    let mut sync = Synchronization::default();
    sync.insert(1, 1, 10, 11).unwrap();
    sync.insert(2, 1, 12, 13).unwrap();
    assert!(!sync.ready(1, 1));
    assert_eq!(sync.commit(1, 1), Err(Error::Busy));
    assert_eq!(sync.signal(2, 1), Ok(12));
    assert_eq!(sync.commit(2, 1), Ok(None));
    assert!(
        !sync.ready(1, 1),
        "other session's acquire remains unsignaled"
    );
    sync.signal(1, 1).unwrap();
    assert_eq!(sync.commit(1, 1), Ok(None));
    assert_eq!(sync.frames[&(1, 1)].stage, Stage::Committed);
    assert_eq!(sync.commit(1, 1), Err(Error::Stale));
    sync.insert(1, 2, 14, 15).unwrap();
    assert_eq!(sync.commit(1, 2), Err(Error::Busy));
    assert!(
        sync.frames.contains_key(&(1, 1)),
        "old buffers still leased"
    );
    sync.signal(1, 2).unwrap();
    let retired = sync.commit(1, 2).unwrap().unwrap();
    assert_eq!((retired.0, retired.1.release), (1, 11));
    assert_eq!(
        sync.commit(1, 3).unwrap().unwrap().1.release,
        15,
        "legacy presents retire predecessor leases"
    );
    sync.validate().unwrap();
}
