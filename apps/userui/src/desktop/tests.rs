use super::*;
use bexos_dioxus_guest::views::ChildView;

fn desktop() -> Desktop {
    let windows = [(2, 102, 128, 100, 618, 446), (1, 100, 40, 32, 560, 400)]
        .into_iter()
        .map(|(id, node, x, y, width, height)| Window {
            view: ChildView {
                id,
                token: id as u32 + 10,
                package: format!("app.{id}"),
                width: 640,
                height: 420,
                focused: false,
            },
            node,
            x,
            y,
            width,
            height,
        })
        .collect();
    Desktop {
        windows,
        next_node: 104,
        width: 800,
        height: 600,
        drag: Some((1, 60.0, 45.0, false)),
        launcher: true,
        ..Default::default()
    }
}

#[test]
fn adoption_preserves_window_geometry_order_and_refs_but_cancels_gestures() {
    let original = desktop();
    let restored = Desktop::restore(&original.checkpoint()).unwrap();
    assert_eq!(restored.next_node, 104);
    assert_eq!((restored.width, restored.height), (800, 600));
    assert!(restored.dirty);
    assert!(restored.drag.is_none());
    assert!(!restored.launcher);
    for (a, b) in original.windows.iter().zip(&restored.windows) {
        assert_eq!((a.x, a.y, a.width, a.height), (b.x, b.y, b.width, b.height));
        assert_eq!(
            (a.view.id, a.view.token, a.node),
            (b.view.id, b.view.token, b.node)
        );
    }
    assert_eq!(restored.windows.len(), 2);
}

#[test]
fn adoption_rejects_aliased_windows_and_node_allocator_collisions() {
    let mut s = desktop();
    s.windows[1].node = s.windows[0].node;
    assert!(Desktop::restore(&s.checkpoint()).is_err());
    let mut s = desktop();
    s.windows[1].view.id = s.windows[0].view.id;
    assert!(Desktop::restore(&s.checkpoint()).is_err());
    let mut s = desktop();
    s.next_node = 102;
    assert!(Desktop::restore(&s.checkpoint()).is_err());
}
