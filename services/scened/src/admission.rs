//! Reserve migration storage before accepting graph growth. Each view can hold
//! pending, committed and three queued graphs. The receiver temporarily keeps
//! both stream bytes and chunks, so graph reservations consume at most 6 MiB of
//! its 8 MiB staging budget, leaving room for input and buffer metadata.
use crate::state::Scene;
use bexos_graphics::scene::Graph;
pub const GRAPH_BUDGET: usize = 3 * 1024 * 1024;
fn weight(graph: &Graph) -> usize {
    graph_weight(
        graph.nodes.len(),
        graph.nodes.values().map(|n| n.style.bytes()).sum(),
    )
}
fn graph_weight(nodes: usize, styles: usize) -> usize {
    // v10 maximum fixed fields, optional layout/clip/content and one parent
    // edge per node, plus root/count and the bounded CSS payload.
    16 + nodes * 280 + styles
}
fn view_reservation(s: &Scene, view: u64) -> usize {
    let current = s.sessions.get(&view).map_or(16, |session| {
        weight(&session.pending).max(weight(&session.committed))
    });
    let largest = s.queues.get(&view).map_or(current, |queue| {
        queue
            .frames
            .iter()
            .fold(current, |n, frame| n.max(weight(&frame.graph)))
    });
    64 + 5 * largest
}
pub fn total(s: &Scene) -> usize {
    s.sessions
        .keys()
        .map(|view| view_reservation(s, *view))
        .sum()
}
/// Largest pending graph the view may grow to without shrinking a committed or
/// queued graph's existing reservation. Zero means legacy imported state has
/// used the budget; removal/release remain available so it can recover.
pub fn available(s: &Scene, view: u64) -> usize {
    let others: usize = s
        .sessions
        .keys()
        .filter(|id| **id != view)
        .map(|id| view_reservation(s, *id))
        .sum();
    GRAPH_BUDGET.saturating_sub(others).saturating_sub(64) / 5
}
pub fn fits(available: usize, nodes: usize, styles: usize) -> bool {
    graph_weight(nodes, styles) <= available
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_reservation_bounds_the_maximum_encoded_graph() {
        let mut graph = Graph::default();
        for id in 1..=256 {
            graph.create(id).unwrap();
            if id != 1 {
                graph.attach(1, id).unwrap();
            }
            let n = graph.node(id).unwrap();
            n.content = Some((
                1,
                bexos_graphics::Surface {
                    width: 1,
                    height: 1,
                    stride: 4,
                    format: bexos_graphics::Format::Rgba,
                },
            ));
            n.clip = Some((1, 1));
            n.layout_size = Some((1., 1.));
            n.embedded = Some(2);
            n.style.identifier = "n".repeat(64);
        }
        graph.root = Some(1);
        let mut w = bexos_migration::codec::Encoder::new();
        crate::state::encode_graph(&graph, &mut w, true, true, true);
        assert!(w.finish().len() <= weight(&graph));
        let mut s = Scene::default();
        for id in 1..=16 {
            let available = available(&s, id);
            if !fits(available, graph.nodes.len(), 256 * 64) {
                break;
            }
            s.sessions.insert(
                id,
                bexos_graphics::scene::Session {
                    pending: graph.clone(),
                    ..Default::default()
                },
            );
            assert!(total(&s) <= GRAPH_BUDGET);
        }
        assert!(s.sessions.len() < 16);
        assert!(!fits(available(&s, 99), 256, 256 * 64));
    }
}
