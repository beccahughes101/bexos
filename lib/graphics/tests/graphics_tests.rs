use bexos_graphics::{ownership::*, progress::*, render::*, scene::*, *};
fn surface() -> Surface {
    Surface {
        width: 320,
        height: 240,
        stride: 1280,
        format: Format::Rgba,
    }
}
#[test]
fn bounds_and_padding() {
    let s = Surface {
        width: 1,
        height: 2,
        stride: 8,
        format: Format::Bgra,
    };
    let mut dst = [99; 16];
    s.copy_rgba(&[1, 2, 3, 255, 4, 5, 6, 255], &mut dst, s.full())
        .unwrap();
    assert_eq!(&dst[..8], &[3, 2, 1, 255, 99, 99, 99, 99]);
    assert_eq!(s.validate(15), Err(Error::Bounds));
    assert!(
        Damage {
            x: u32::MAX,
            y: 0,
            width: 2,
            height: 1
        }
        .validate(s)
        .is_err()
    );
}
#[test]
fn progress_is_monotonic() {
    let mut p = Progress::default();
    assert!(p.report(3, 40, "drivers").unwrap());
    assert!(!p.report(2, 30, "late").unwrap());
    assert_eq!(p.percent, 40);
    assert!(p.report(4, 101, "").is_err());
}
#[test]
fn skips_late_frames() {
    let mut clock = FrameClock::default();
    assert!(clock.due(0));
    assert!(!clock.due(1));
    assert!(clock.due(1_000_000));
    assert!(!clock.due(1_000_001));
}
#[test]
fn transfer_rollback_rejects_late_ack() {
    let mut o = Ownership::default();
    let g = o.acquire(1).unwrap();
    let next = o.begin(1, g, 2, 0).unwrap();
    assert!(o.authorize(1, g).is_err());
    assert!(o.expire(2_000_000));
    assert!(o.complete(2, next).is_err());
    assert_eq!(o.owner, 1);
    assert!(o.generation > next);
}
#[test]
fn transfer_completion_and_peer_death() {
    let mut o = Ownership::default();
    let g = o.acquire(1).unwrap();
    let next = o.begin(1, g, 2, 0).unwrap();
    o.complete(2, next).unwrap();
    assert!(!o.expire(u64::MAX));
    o.disconnected(2);
    assert_eq!(o.owner, 0);
}
#[test]
fn atomic_present_and_isolation() {
    let mut a = Session::default();
    a.pending.create(1).unwrap();
    a.pending.root = Some(1);
    a.present(1).unwrap();
    a.pending.node(1).unwrap().opacity = f32::NAN;
    assert!(a.present(2).is_err());
    assert_eq!(a.committed.nodes[&1].opacity, 1.);
    assert!(Session::default().pending.node(1).is_err());
}
#[test]
fn rejects_cycles() {
    let mut g = Graph::default();
    g.create(1).unwrap();
    g.create(2).unwrap();
    g.attach(1, 2).unwrap();
    assert!(g.attach(2, 1).is_err());
}
#[test]
fn deterministic_animation_and_exact_handoff() {
    let mut r = Renderer::new(surface()).unwrap();
    let first = r.splash(0, 25).to_vec();
    assert_eq!(first, r.splash(0, 25));
    assert_ne!(first, r.splash(100_000, 25));
    let mut out = vec![0; first.len()];
    cross_fade(&first, &mut out, 0).unwrap();
    assert_eq!(first, out);
    cross_fade(&first, &mut out, 250_000).unwrap();
    assert_eq!(&out[..4], &[14, 18, 28, 255]);
}
#[test]
fn alternating_back_buffers_include_previous_damage() {
    let s = Surface {
        width: 2,
        height: 1,
        stride: 8,
        format: Format::Rgba,
    };
    let mut a = [0; 8];
    let mut b = [0; 8];
    let first = [1, 2, 3, 255, 0, 0, 0, 0];
    assert_eq!(
        buffer::update_back_buffer(s, &a, &mut b, s, &first, s.full()).unwrap(),
        Some(Damage {
            x: 0,
            y: 0,
            width: 1,
            height: 1
        })
    );
    let second = [0, 0, 0, 0, 4, 5, 6, 255];
    let dirty = buffer::update_back_buffer(
        s,
        &b,
        &mut a,
        s,
        &second,
        Damage {
            x: 1,
            y: 0,
            width: 1,
            height: 1,
        },
    )
    .unwrap();
    assert_eq!(dirty, Some(s.full()));
    assert_eq!(a, [1, 2, 3, 255, 4, 5, 6, 255]);
    assert_eq!(
        buffer::update_back_buffer(s, &a, &mut b, s, &a, s.full()).unwrap(),
        Some(Damage {
            x: 1,
            y: 0,
            width: 1,
            height: 1
        })
    );
}
#[test]
fn duplicate_transfer_and_peer_death_never_restore_stale_authority() {
    let mut o = Ownership::default();
    let g = o.acquire(4).unwrap();
    let next = o.begin(4, g, 5, 10).unwrap();
    assert!(o.begin(4, g, 5, 11).is_err());
    o.disconnected(5);
    assert_eq!(o.owner, 4);
    assert!(o.authorize(4, g).is_err());
    assert!(o.complete(5, next).is_err());
}

#[test]
fn nested_transforms_clip_and_blend_across_pixel_formats() {
    let target = Surface {
        width: 4,
        height: 4,
        stride: 16,
        format: Format::Bgra,
    };
    let content = Surface {
        width: 1,
        height: 1,
        stride: 4,
        format: Format::Rgba,
    };
    let mut g = Graph::default();
    g.create(1).unwrap();
    g.create(2).unwrap();
    g.attach(1, 2).unwrap();
    g.root = Some(1);
    let root = g.node(1).unwrap();
    root.translation = (1, 1);
    root.scale = (2., 2.);
    root.clip = Some((1, 1));
    let child = g.node(2).unwrap();
    child.content = Some((7, content));
    child.opacity = 0.5;
    let mut pixels = [0u8; 64];
    for p in pixels.chunks_exact_mut(4) {
        p[3] = 255;
    }
    composite(&g, target, &mut pixels, |handle| {
        (handle == 7).then(|| vec![100, 50, 20, 255])
    })
    .unwrap();
    for y in 0..4 {
        for x in 0..4 {
            let expected = if (1..3).contains(&x) && (1..3).contains(&y) {
                [10, 25, 50, 255]
            } else {
                [0, 0, 0, 255]
            };
            assert_eq!(&pixels[(y * 4 + x) * 4..(y * 4 + x + 1) * 4], &expected);
        }
    }
}

#[test]
fn tracked_swapchain_repairs_previous_updates_and_preserves_padding() {
    let surface = Surface {
        width: 4,
        height: 2,
        stride: 20,
        format: Format::Bgra,
    };
    let mut front = [0u8; 40];
    let mut back = [0u8; 40];
    let mut source = [0u8; 40];
    front[0..4].copy_from_slice(&[1, 2, 3, 255]);
    source[24..28].copy_from_slice(&[4, 5, 6, 255]);
    back[16..20].fill(77);
    let old = bexos_graphics::Damage {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    };
    let new = bexos_graphics::Damage {
        x: 1,
        y: 1,
        width: 1,
        height: 1,
    };
    let damage = bexos_graphics::buffer::update_tracked_back_buffer(
        surface,
        &front,
        &mut back,
        surface,
        &source,
        Some(old),
        new,
    )
    .unwrap();
    assert_eq!(damage, old.union(new));
    assert_eq!(&back[0..4], &[1, 2, 3, 255]);
    assert_eq!(&back[24..28], &[4, 5, 6, 255]);
    assert_eq!(&back[16..20], &[77; 4]);
}

#[test]
fn late_wakeups_preserve_frame_phase_at_both_refresh_rates() {
    for period in [8333, 16667] {
        let mut clock = FrameClock::default();
        assert!(clock.due_with_period(100, period));
        assert!(clock.due_with_period(100 + 3 * period + 17, period));
        assert_eq!(clock.next_us, 100 + 4 * period);
        assert!(!clock.due_with_period(clock.next_us - 1, period));
        let before = clock;
        assert!(!clock.due_with_period(u64::MAX, 0));
        assert_eq!(before, clock);
    }
}
