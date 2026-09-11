use bexos_flatland::{Format, Surface, scene::Graph};
use bexos_flatland_layout::graph::Cache;
fn target() -> Surface {
    Surface {
        width: 1920,
        height: 1080,
        stride: 7680,
        format: Format::Bgra,
    }
}
#[test]
fn changing_parent_mode_repositions_children_and_leaves_pending_untouched() {
    let mut graph = Graph::default();
    for id in 1..=3 {
        graph.create(id).unwrap();
    }
    graph.root = Some(1);
    graph.attach(1, 2).unwrap();
    graph.attach(1, 3).unwrap();
    for id in [2, 3] {
        graph.node(id).unwrap().layout.width = 100.;
        graph.node(id).unwrap().layout.height = 50.;
    }
    let mut cache = Cache::default();
    let manual = cache.prepare(&graph, target()).unwrap();
    assert_eq!(manual.nodes[&3].layout_offset, (0., 0.));
    graph.node(1).unwrap().layout.mode = 1;
    let row = cache.prepare(&graph, target()).unwrap();
    assert_eq!(row.nodes[&3].layout_offset, (100., 0.));
    assert_eq!(graph.nodes[&3].layout_offset, (0., 0.));
    graph.node(1).unwrap().layout.mode = 2;
    let column = cache.prepare(&graph, target()).unwrap();
    assert_eq!(column.nodes[&3].layout_offset, (0., 50.));
}
#[test]
fn grid_resize_and_removal_rebuild_valid_topology() {
    let mut graph = Graph::default();
    for id in 1..=5 {
        graph.create(id).unwrap();
    }
    graph.root = Some(1);
    graph.node(1).unwrap().layout.mode = 3;
    graph.node(1).unwrap().layout.columns = 2;
    for id in 2..=5 {
        graph.attach(1, id).unwrap();
        graph.node(id).unwrap().layout.height = 20.;
    }
    let mut cache = Cache::default();
    let grid = cache.prepare(&graph, target()).unwrap();
    assert_eq!(grid.nodes[&3].layout_offset.0, 960.);
    graph.remove(3).unwrap();
    let changed = cache.prepare(&graph, target()).unwrap();
    assert_eq!(changed.nodes[&4].layout_offset.0, 960.);
}
