use bexos_flatland::{
    resolved::Snapshot,
    scene::{Graph, Session},
    world::{Paint, World},
    *,
};
use std::collections::BTreeMap;
fn surface() -> Surface {
    Surface {
        width: 100,
        height: 100,
        stride: 400,
        format: Format::Rgba,
    }
}
fn graph() -> Graph {
    let mut g = Graph::default();
    g.create(1).unwrap();
    g.root = Some(1);
    g.node(1).unwrap().content = Some((1, surface()));
    g
}

#[test]
fn fullscreen_root_hides_embedded_views_then_restores_cached_geometry() {
    let mut parent = graph();
    parent.node(1).unwrap().embedded = Some(2);
    parent.create(3).unwrap();
    parent.node(3).unwrap().content = Some((
        3,
        Surface {
            format: Format::Bgrx,
            ..surface()
        },
    ));
    let mut sessions = BTreeMap::from([
        (
            1,
            Session {
                committed: parent,
                ..Default::default()
            },
        ),
        (
            2,
            Session {
                committed: graph(),
                ..Default::default()
            },
        ),
    ]);
    let mut snapshots = BTreeMap::<u64, Snapshot>::new();
    let mut world = World::default();
    world
        .compile(&sessions, &[1], surface(), &mut snapshots)
        .unwrap();
    let child = snapshots[&2].items.clone();
    sessions.get_mut(&1).unwrap().committed.root = Some(3);
    world
        .compile(&sessions, &[1], surface(), &mut snapshots)
        .unwrap();
    assert!(snapshots[&2].items.is_empty());
    let candidate = scanout::candidate(
        world.items(&snapshots).map(|(_, item)| item),
        surface(),
        1 << 3,
        false,
    )
    .unwrap();
    assert_eq!(candidate.buffer, 3);
    assert_eq!(world.paint, [Paint { view: 1, item: 0 }]);
    sessions.get_mut(&1).unwrap().committed.root = Some(1);
    world
        .compile(&sessions, &[1], surface(), &mut snapshots)
        .unwrap();
    assert_eq!(snapshots[&2].items, child);
    assert_eq!(
        world.paint,
        [Paint { view: 1, item: 0 }, Paint { view: 2, item: 0 }]
    );
}
#[test]
fn embedded_view_inherits_geometry_and_interleaves_with_parent_content() {
    let mut parent = graph();
    parent.node(1).unwrap().translation = (10, 20);
    parent.node(1).unwrap().scale = (2., 2.);
    parent.node(1).unwrap().clip = Some((30, 20));
    parent.node(1).unwrap().embedded = Some(2);
    parent.create(2).unwrap();
    parent.node(2).unwrap().content = Some((3, surface()));
    parent.attach(1, 2).unwrap();
    let mut child = graph();
    child.node(1).unwrap().translation = (5, 2);
    child.node(1).unwrap().content = Some((2, surface()));
    let sessions = BTreeMap::from([
        (
            1,
            Session {
                committed: parent,
                ..Default::default()
            },
        ),
        (
            2,
            Session {
                committed: child,
                ..Default::default()
            },
        ),
    ]);
    let mut snapshots = BTreeMap::<u64, Snapshot>::new();
    let mut world = World::default();
    world
        .compile(&sessions, &[1], surface(), &mut snapshots)
        .unwrap();
    assert_eq!(
        world.paint,
        [
            Paint { view: 1, item: 0 },
            Paint { view: 2, item: 0 },
            Paint { view: 1, item: 1 }
        ]
    );
    let item = &snapshots[&2].items[0];
    assert_eq!(
        (item.transform.x, item.transform.y, item.transform.sx),
        (20., 24., 2.)
    );
    assert_eq!(item.local(24., 30.), (2., 3.));
    assert_eq!((item.visible.width, item.visible.height), (50., 36.));
    assert!(!item.visible.contains(30., 61.));
    let mut pixels = vec![0; 40000];
    let colors = [
        [255, 0, 0, 255].repeat(10000),
        [0, 255, 0, 255].repeat(10000),
        [0, 0, 255, 255].repeat(10000),
    ];
    composition::composite_items(
        world.items(&snapshots).map(|(_, i)| i),
        surface(),
        &mut pixels,
        surface().full(),
        |id| colors.get(id as usize - 1),
    )
    .unwrap();
    assert_eq!(
        &pixels[(30 * 100 + 24) * 4..(30 * 100 + 24) * 4 + 4],
        &[0, 0, 255, 255]
    );
    assert_eq!(
        world
            .items(&snapshots)
            .rev()
            .find(|(_, i)| i.visible.contains(24., 30.))
            .unwrap()
            .0,
        1
    );
}
#[test]
fn cycles_multiple_parents_and_nonfinite_inherited_geometry_are_atomic() {
    let mut a = graph();
    let mut b = graph();
    a.node(1).unwrap().embedded = Some(2);
    let mut sessions = BTreeMap::from([
        (
            1,
            Session {
                committed: a,
                ..Default::default()
            },
        ),
        (
            2,
            Session {
                committed: b.clone(),
                ..Default::default()
            },
        ),
    ]);
    let mut snapshots = BTreeMap::new();
    let mut world = World::default();
    world
        .compile(&sessions, &[1], surface(), &mut snapshots)
        .unwrap();
    let previous = world.paint.clone();
    b.node(1).unwrap().embedded = Some(1);
    sessions.get_mut(&2).unwrap().committed = b;
    assert!(
        world
            .compile(&sessions, &[1], surface(), &mut snapshots)
            .is_err()
    );
    assert_eq!(world.paint, previous);
    sessions
        .get_mut(&2)
        .unwrap()
        .committed
        .node(1)
        .unwrap()
        .embedded = None;
    sessions.insert(
        3,
        Session {
            committed: sessions[&1].committed.clone(),
            ..Default::default()
        },
    );
    assert!(World::validate(&sessions).is_err());
    sessions.remove(&3);
    let mut deep = Graph::default();
    for id in 1..=200 {
        deep.create(id).unwrap();
        deep.node(id).unwrap().scale = (64., 64.);
        if id > 1 {
            deep.node(id - 1).unwrap().children.push(id);
        }
    }
    deep.root = Some(1);
    // Each local tree is finite, but nesting both exceeds the global depth limit.
    for node in deep.nodes.values_mut() {
        node.scale = (2., 2.);
    }
    sessions.get_mut(&1).unwrap().committed = deep.clone();
    sessions.get_mut(&2).unwrap().committed = deep;
    sessions
        .get_mut(&1)
        .unwrap()
        .committed
        .node(200)
        .unwrap()
        .embedded = Some(2);
    assert!(
        world
            .compile(&sessions, &[1], surface(), &mut snapshots)
            .is_err()
    );
    assert_eq!(world.paint, previous);
}
#[test]
fn hiding_parent_clears_child_geometry_used_for_input() {
    let mut parent = graph();
    parent.node(1).unwrap().embedded = Some(2);
    let mut sessions = BTreeMap::from([
        (
            1,
            Session {
                committed: parent,
                ..Default::default()
            },
        ),
        (
            2,
            Session {
                committed: graph(),
                ..Default::default()
            },
        ),
    ]);
    let mut snapshots = BTreeMap::new();
    let mut world = World::default();
    world
        .compile(&sessions, &[1], surface(), &mut snapshots)
        .unwrap();
    assert_eq!(world.paint.len(), 2);
    sessions
        .get_mut(&1)
        .unwrap()
        .committed
        .node(1)
        .unwrap()
        .opacity = 0.;
    world
        .compile(&sessions, &[1], surface(), &mut snapshots)
        .unwrap();
    assert!(world.paint.is_empty());
    assert!(snapshots[&2].items.is_empty());
    sessions
        .get_mut(&1)
        .unwrap()
        .committed
        .node(1)
        .unwrap()
        .opacity = 1.;
    world
        .compile(&sessions, &[1], surface(), &mut snapshots)
        .unwrap();
    assert_eq!(world.paint.len(), 2);
}
