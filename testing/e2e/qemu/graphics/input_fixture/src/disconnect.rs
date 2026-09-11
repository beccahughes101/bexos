//! F8 closes the embedded view while the other client remains operational.
use bexos_graphics_runtime as rt;
use bexos_userspace::{Channel, Memory};
use graphics_fidl::*;
pub fn start(f: &mut crate::Fixture) {
    if f.transition.phase != 4 {
        return;
    }
    Memory::close(f.sessions[1].0).unwrap();
    f.sessions[1] = Channel(0);
    f.transition.phase = 5;
}
pub fn poll(f: &mut crate::Fixture) {
    if f.transition.phase != 5 {
        return;
    }
    let Ok(message) = Channel(f.releases[1]).try_recv() else {
        return;
    };
    assert!(message.handles.is_empty());
    let release = PresentationRelease::decode(&message.bytes, &[]).unwrap();
    assert_eq!(release.sequence, 1);
    assert_eq!(release.status, Status::ErrIo);
    Memory::close(f.releases[1]).unwrap();
    f.releases[1] = 0;
    let reply: AccessibilityControlGetViewGeometryResponse = rt::call(
        &mut Channel(f.accessibility.control),
        1,
        &AccessibilityControlGetViewGeometryRequest {
            token: HandleRef {
                raw: Memory::duplicate(f.accessibility.tokens[1], 1 | 32).unwrap(),
            },
        },
    )
    .unwrap();
    assert_eq!(reply.status, Status::ErrAccessDenied);
    // A request to the surviving client must still complete after peer teardown.
    let info = rt::flatland::Session::new(f.sessions[0])
        .feedback()
        .unwrap();
    assert_eq!(info.status, Status::Ok);
    f.transition.phase = 6;
    bexos_userspace::log(
        "input-fixture: disconnected view retired lease and identity; surviving client responsive\n",
    );
}
