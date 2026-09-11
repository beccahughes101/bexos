use bexos_flatland::{Format, Surface, resolved::Snapshot, scene::Graph};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
#[test]
fn retained_geometry_updates_do_not_allocate_after_warmup() {
    let surface = Surface {
        width: 1920,
        height: 1080,
        stride: 7680,
        format: Format::Bgra,
    };
    let mut g = Graph::default();
    for id in 1..=100 {
        g.create(id).unwrap();
        g.node(id).unwrap().content = Some((id, surface));
        if id > 1 {
            g.attach(1, id).unwrap();
        }
    }
    g.root = Some(1);
    let mut snapshot = Snapshot::default();
    snapshot.compile(&g, surface).unwrap();
    snapshot.compile(&g, surface).unwrap();
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    for frame in 0..200 {
        g.node(2).unwrap().translation = (frame + 1, 0);
        snapshot.compile(std::hint::black_box(&g), surface).unwrap();
        assert_eq!(snapshot.recomputed_nodes(), 1);
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
    assert_eq!(allocations, 0);
    // Rebuild inter-view paint order without allocating or re-resolving the
    // unaffected 799 nodes in an eight-view embedded hierarchy.
    use bexos_flatland::{scene::Session, world::World};
    let mut sessions = std::collections::BTreeMap::new();
    for id in 1..=8 {
        let mut graph = g.clone();
        if id < 8 {
            graph.node(1).unwrap().embedded = Some(id + 1);
        }
        sessions.insert(
            id,
            Session {
                committed: graph,
                ..Default::default()
            },
        );
    }
    let mut snapshots = std::collections::BTreeMap::new();
    let mut world = World::default();
    for _ in 0..2 {
        world
            .compile(&sessions, &[1], surface, &mut snapshots)
            .unwrap();
    }
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    for frame in 0..200 {
        sessions
            .get_mut(&8)
            .unwrap()
            .committed
            .node(50)
            .unwrap()
            .translation = (frame + 1, 0);
        world
            .compile(&sessions, &[1], surface, &mut snapshots)
            .unwrap();
        assert_eq!(
            snapshots
                .values()
                .map(|s| s.recomputed_nodes())
                .sum::<usize>(),
            1
        );
    }
    assert_eq!(ALLOCATIONS.load(Ordering::Relaxed) - before, 0);
}
