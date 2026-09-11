use bexos_graphics::scene::Session;
use bexos_graphics_runtime::migration::Runtime;
use bexos_scened::state::Scene;
use bexos_userspace::{Channel, live_migration::State};
fn legacy_scene(version: u64) -> Scene {
    let mut scene = Scene::default();
    scene.records_version = version;
    scene.legacy_styles = true;
    scene.legacy_effects = true;
    scene.legacy_graphs = version < 5;
    scene.legacy_presentations = true;
    scene.legacy_controls = version < 4;
    scene.legacy_fences = version < 3;
    scene.scanout.legacy = true;
    scene.gpu.legacy = true;
    scene
}
#[test]
fn version_five_source_does_not_require_a_display_submission_record() {
    let source = Runtime::new(Channel(1), Some(Channel(2)), legacy_scene(5));
    let keys = source.keys();
    let mut target = Runtime::<Scene>::empty();
    for key in &keys {
        let bytes = source.encode_record(*key).unwrap().unwrap();
        target.adopt_record(*key, Some(&bytes)).unwrap();
    }
    target.finish_adoption().unwrap();
    assert_eq!(target.keys(), keys);
    assert!(target.component.pending_frame.is_none());
    assert!(target.component.legacy_presentations);
}

#[test]
fn malformed_takeover_retry_record_does_not_replace_the_candidate_state() {
    let mut source = Runtime::new(Channel(1), Some(Channel(2)), Scene::default());
    source.component.takeover_deadline_us = 100;
    source.component.takeover_retry_us = 50;
    let mut target = Runtime::<Scene>::empty();
    let valid = source.encode_record(4).unwrap().unwrap();
    target.adopt_record(4, Some(&valid)).unwrap();
    source.component.takeover_retry_us = 101;
    let invalid = source.encode_record(4).unwrap().unwrap();
    assert!(target.adopt_record(4, Some(&invalid)).is_err());
    assert_eq!(target.component.takeover_retry_us, 50);
    for len in 0..valid.len() {
        assert!(target.adopt_record(4, Some(&valid[..len])).is_err());
    }
}
#[test]
fn pending_display_submission_survives_global_deltas_with_its_original_sequences() {
    use bexos_graphics_runtime::presentation::Submission;
    use bexos_scened::presentation::PendingFrame;
    let mut scene = Scene::default();
    scene.takeover_deadline_us = 2_000_000;
    scene.takeover_retry_us = 50_000;
    scene.canvas = Some(bexos_graphics_runtime::canvas::Canvas {
        display: Channel(4),
        surface: bexos_graphics::Surface {
            width: 8,
            height: 8,
            stride: 32,
            format: bexos_graphics::Format::Bgra,
        },
        output: bexos_graphics_runtime::Mapping {
            handle: 5,
            address: 4096,
            size: 256,
            rights: 6,
            owned: false,
        },
        generation: 1,
        client_id: 9,
    });
    let mut sequences = [(0, 0); 16];
    sequences[0] = (10, 7);
    sequences[1] = (20, 3);
    scene.pending_frame = Some(PendingFrame {
        scanout_buffer: 0,
        submission: Submission {
            deadline_us: 2_000_100,
        },
        frame_time_us: 100,
        timer_deadline_us: 8_333,
        frozen: false,
        resumed: true,
        sequences,
        count: 2,
    });
    let source = Runtime::new(Channel(1), Some(Channel(2)), scene);
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
    assert_eq!(target.component.takeover_deadline_us, 2_000_000);
    assert_eq!(target.component.takeover_retry_us, 50_000);
    let frame = target.component.pending_frame.as_ref().unwrap();
    assert_eq!(frame.sequences[..frame.count], [(10, 7), (20, 3)]);
    assert_eq!(frame.submission.deadline_us, 2_000_100);
    let encoded = PendingFrame::encode(Some(frame));
    for len in 0..encoded.len() {
        assert!(PendingFrame::decode(&encoded[..len]).is_err());
    }
    let mut duplicate = frame.clone();
    duplicate.sequences[1].0 = 10;
    assert!(PendingFrame::decode(&PendingFrame::encode(Some(&duplicate))).is_err());
}
#[test]
fn desktop_renders_multilingual_text_and_respects_damage() {
    use bexos_graphics::{Damage, Format, Surface};
    let surface = Surface {
        width: 800,
        height: 600,
        stride: 3200,
        format: Format::Bgra,
    };
    let desktop = bexos_scened::desktop::Desktop::new(surface).unwrap();
    let mut pixels = vec![0; surface.validate(u64::MAX).unwrap()];
    desktop
        .compose(
            surface,
            &mut pixels,
            Damage {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
        )
        .unwrap();
    assert!(pixels.iter().all(|b| *b == 0));
    desktop
        .compose(surface, &mut pixels, surface.full())
        .unwrap();
    assert!(pixels.chunks_exact(4).filter(|p| p[3] > 128).count() > 500);
}
#[test]
fn desktop_configuration_rejects_malformed_and_unbounded_values() {
    use bexos_scened::desktop_config::DesktopConfig;
    // text="a", size=24, packed RGBA white.
    let bytes = [10, 1, b'a', 16, 24, 29, 255, 255, 255, 255];
    let config = DesktopConfig::decode(&bytes).unwrap();
    assert_eq!(config.font_size, 24);
    assert_eq!(config.insets, [0; 4]);
    for n in 0..bytes.len() {
        assert!(DesktopConfig::decode(&bytes[..n]).is_err());
    }
    let mut duplicate = bytes.to_vec();
    duplicate.extend_from_slice(&[16, 25]);
    assert!(DesktopConfig::decode(&duplicate).is_err());
    let mut oversized = bytes.to_vec();
    oversized.extend_from_slice(&[32, 0xff, 0x7f]);
    assert!(DesktopConfig::decode(&oversized).is_err());
}

#[test]
fn gpu_profiling_is_opt_in_and_rejects_non_boolean_configuration() {
    use bexos_scened::desktop_config::DesktopConfig;
    let bytes = [10, 1, b'a', 16, 24, 29, 255, 255, 255, 255];
    assert!(!DesktopConfig::decode(&bytes).unwrap().profile_gpu);
    for (value, expected) in [(0, Some(false)), (1, Some(true)), (2, None)] {
        let mut config = bytes.to_vec();
        config.extend([80, value]);
        assert_eq!(
            DesktopConfig::decode(&config).ok().map(|c| c.profile_gpu),
            expected
        );
    }
}

#[test]
fn software_vulkan_recovery_configuration_is_bounded_and_backward_compatible() {
    use bexos_scened::desktop_config::DesktopConfig;
    let bytes = [10, 1, b'a', 16, 24, 29, 255, 255, 255, 255];
    assert_eq!(
        DesktopConfig::decode(&bytes)
            .unwrap()
            .software_vulkan_timeout_ms,
        2000
    );
    for (value, expected) in [
        (0u32, Some(2000)),
        (1999, None),
        (2000, Some(2000)),
        (30000, Some(30000)),
        (30001, None),
        (u32::MAX, None),
    ] {
        let mut config = bytes.to_vec();
        config.push(88);
        let mut remaining = value;
        while remaining > 127 {
            config.push((remaining as u8 & 127) | 128);
            remaining >>= 7;
        }
        config.push(remaining as u8);
        assert_eq!(
            DesktopConfig::decode(&config)
                .ok()
                .map(|c| c.software_vulkan_timeout_ms),
            expected
        );
        config.extend([88, 0]);
        assert!(DesktopConfig::decode(&config).is_err());
    }
}

#[test]
fn fence_records_preserve_waiting_and_committed_leases_across_deltas() {
    use bexos_graphics::synchronization::Stage;
    let mut s = Scene::default();
    let mut session = Session::default();
    session.pending.create(1).unwrap();
    session.pending.root = Some(1);
    session.committed = session.pending.clone();
    s.sessions.insert(10, session);
    let queue = s.queues.entry(10).or_default();
    queue.submit(&s.sessions[&10].pending, 0).unwrap();
    queue.submit(&s.sessions[&10].pending, 1).unwrap();
    queue.latch(0).unwrap();
    s.fences.insert(10, 1, 100, 101).unwrap();
    s.fences.signal(10, 1).unwrap();
    s.fences.commit(10, 1).unwrap();
    s.fences.insert(10, 2, 102, 103).unwrap();
    let source = Runtime::new(Channel(1), Some(Channel(2)), s);
    let mut target = Runtime::<Scene>::empty();
    for key in source.keys() {
        target
            .adopt_record(key, source.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    // A later global delta must not erase separately adopted fence state.
    target
        .adopt_record(1, source.encode_record(1).unwrap().as_deref())
        .unwrap();
    target.finish_adoption().unwrap();
    assert_eq!(
        target.component.fences.frames[&(10, 1)].stage,
        Stage::Committed
    );
    assert_eq!(target.component.fences.frames[&(10, 2)].acquire, Some(102));
    assert_eq!(target.component.fences.handles(), [101, 102, 103]);
    let mut malformed = bexos_scened::fences::encode(&source.component.fences);
    malformed.push(0);
    assert!(bexos_scened::fences::decode(&malformed).is_err());
}

#[test]
fn chunked_version_two_import_preserves_original_record_keys_until_activation() {
    let source = Runtime::new(Channel(1), Some(Channel(2)), legacy_scene(2));
    let mut target = Runtime::<Scene>::empty();
    let keys = source.keys();
    for key in &keys {
        let bytes = source.encode_record(*key).unwrap().unwrap();
        target.adopt_record(*key, Some(&bytes)).unwrap();
    }
    target.finish_adoption().unwrap();
    assert_eq!(target.keys(), keys);
    assert!(target.component.legacy_fences);
}
#[test]
fn queued_presentations_survive_transplant_without_early_latch() {
    let mut scene = Scene::default();
    let mut session = Session::default();
    session.pending.create(1).unwrap();
    session.pending.root = Some(1);
    let mut queue = bexos_graphics::presentation::Queue::default();
    queue.submit(&session.pending, 100).unwrap();
    session.pending.node(1).unwrap().translation = (20, 0);
    queue.submit(&session.pending, 200).unwrap();
    scene.sessions.insert(4, session);
    scene.queues.insert(4, queue);
    let source = Runtime::new(Channel(1), Some(Channel(2)), scene);
    let mut target = Runtime::<Scene>::empty();
    for key in source.keys() {
        target
            .adopt_record(key, source.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    target.finish_adoption().unwrap();
    let q = target.component.queues.get_mut(&4).unwrap();
    assert!(q.latch(99).is_none());
    assert_eq!(q.latch(100).unwrap().graph.nodes[&1].translation, (0, 0));
    assert_eq!(q.latch(200).unwrap().graph.nodes[&1].translation, (20, 0));
}
#[test]
fn legacy_monolithic_source_imports_without_new_record_keys() {
    use bexos_graphics_runtime::migration::Component;
    let mut scene = Scene::default();
    let mut session = Session::default();
    session.pending.create(1).unwrap();
    scene.sessions.insert(4, session);
    let source = Runtime::new(Channel(1), Some(Channel(2)), scene);
    let mut target = Runtime::<Scene>::empty();
    target
        .adopt_record(0, source.encode_record(0).unwrap().as_deref())
        .unwrap();
    let mut w = bexos_migration::codec::Encoder::new();
    w.word(1);
    source.component.encode(&mut w).unwrap();
    target.adopt_record(1, Some(&w.finish())).unwrap();
    target.finish_adoption().unwrap();
    assert_eq!(target.keys(), vec![0, 1]);
    assert!(target.component.sessions[&4].pending.nodes.contains_key(&1));
}
#[test]
fn transplant_retains_pending_and_committed_scene() {
    let mut scene = Scene::default();
    let mut session = Session::default();
    session.pending.create(1).unwrap();
    session.pending.root = Some(1);
    session.present(10).unwrap();
    session.pending.node(1).unwrap().translation = (12, -3);
    scene.sessions.insert(4, session);
    let old = Runtime::new(Channel(1), Some(Channel(2)), scene);
    let mut new = Runtime::<Scene>::empty();
    for key in old.keys() {
        new.adopt_record(key, old.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    new.finish_adoption().unwrap();
    assert_eq!(
        new.component.sessions[&4].pending.nodes[&1].translation,
        (12, -3)
    );
    assert_eq!(
        new.component.sessions[&4].committed.nodes[&1].translation,
        (0, 0)
    );
}
#[test]
fn malformed_candidate_does_not_change_source() {
    let old = Runtime::new(Channel(1), Some(Channel(2)), Scene::default());
    let before = old.encode_record(1).unwrap();
    let mut new = Runtime::<Scene>::empty();
    assert!(new.adopt_record(1, Some(&[0; 8])).is_err());
    assert_eq!(before, old.encode_record(1).unwrap());
}
#[test]
fn transplant_preserves_frozen_frame_and_fade_timing() {
    use bexos_graphics::{Format, Surface, progress::FrameClock};
    use bexos_graphics_runtime::{Mapping, canvas::Canvas};
    let scene = Scene {
        canvas: Some(Canvas {
            display: Channel(4),
            surface: Surface {
                width: 8,
                height: 8,
                stride: 32,
                format: Format::Bgra,
            },
            output: Mapping {
                handle: 5,
                address: 4096,
                size: 256,
                rights: 6,
                owned: false,
            },
            generation: 7,
            client_id: 9,
        }),
        frozen: Some(Mapping {
            handle: 6,
            address: 8192,
            size: 256,
            rights: 2,
            owned: false,
        }),
        start_us: 1234,
        clock: FrameClock { next_us: 18000 },
        ready: true,
        ..Default::default()
    };
    let old = Runtime::new(Channel(1), Some(Channel(2)), scene);
    let mut new = Runtime::<Scene>::empty();
    for k in old.keys() {
        new.adopt_record(k, old.encode_record(k).unwrap().as_deref())
            .unwrap();
    }
    new.finish_adoption().unwrap();
    assert_eq!(new.component.start_us, 1234);
    assert_eq!(new.component.clock.next_us, 18000);
    assert_eq!(new.component.frozen.unwrap().rights, 2);
    assert_eq!(new.component.canvas.unwrap().generation, 7);
}

#[test]
fn largest_admitted_scene_uses_bounded_migration_records() {
    use bexos_graphics::{Format, Surface, presentation::Queue};
    use bexos_graphics_runtime::Mapping;
    use bexos_scened::admission;
    let mut scene = Scene::default();
    // Reserve all session headers before filling their graphs through the same
    // check used by CreateTransform, including five copies per live view.
    for id in 1..=16 {
        scene.sessions.insert(id, Session::default());
    }
    let mut nodes = 0;
    for session_id in 1..=16 {
        let mut session = Session::default();
        let available = admission::available(&scene, session_id);
        for id in 1..=256 {
            if !admission::fits(available, id as usize, 0) {
                break;
            }
            session.pending.create(id).unwrap();
            nodes += 1;
            let h = 1000 + session_id * 32 + id % 32;
            let node = session.pending.node(id).unwrap();
            node.clip = Some((8, 8));
            node.content = Some((
                h,
                Surface {
                    width: 8,
                    height: 8,
                    stride: 32,
                    format: Format::Bgra,
                },
            ));
            scene.buffers.insert(
                h,
                Mapping {
                    handle: h,
                    address: h * 4096,
                    size: 256,
                    rights: 2,
                    owned: false,
                },
            );
            if id > 1 {
                session.pending.attach(1, id).unwrap();
            }
        }
        session.pending.root = session.pending.nodes.contains_key(&1).then_some(1);
        session.present(1).unwrap();
        let mut queue = Queue::default();
        for time in 2..=4 {
            queue.submit(&session.pending, time).unwrap();
        }
        scene.queues.insert(session_id, queue);
        scene.sessions.insert(session_id, session);
    }
    assert!(admission::total(&scene) <= admission::GRAPH_BUDGET);
    assert!(nodes > 2048 && nodes < 4096);
    assert!(!admission::fits(admission::available(&scene, 16), 1, 0));
    let buffers = scene.buffers.len();
    let state = Runtime::new(Channel(1), Some(Channel(2)), scene);
    state.validate().unwrap();
    let mut target = Runtime::<Scene>::empty();
    let mut bytes = 0;
    for key in state.keys() {
        let record = state.encode_record(key).unwrap().unwrap();
        assert!(record.len() <= 32704);
        bytes += record.len();
        target.adopt_record(key, Some(&record)).unwrap();
    }
    assert!(bytes < 8 * 1024 * 1024);
    target.finish_adoption().unwrap();
    assert_eq!(target.component.buffers.len(), buffers);
    assert_eq!(
        target
            .component
            .sessions
            .values()
            .map(|s| s.pending.nodes.len())
            .sum::<usize>(),
        nodes
    );
    assert_eq!(
        target
            .component
            .queues
            .values()
            .map(|q| q.frames.len())
            .sum::<usize>(),
        48
    );
}
#[test]
fn damaged_or_missing_chunk_rejects_candidate() {
    let mut scene = Scene::default();
    let mut session = Session::default();
    session.pending.create(1).unwrap();
    scene.sessions.insert(4, session);
    let state = Runtime::new(Channel(1), Some(Channel(2)), scene);
    for corrupt in [false, true] {
        let mut target = Runtime::<Scene>::empty();
        for key in state.keys() {
            let mut bytes = state.encode_record(key).unwrap().unwrap();
            if key > 1 && key as u32 == 1 {
                if !corrupt {
                    continue;
                }
                *bytes.last_mut().unwrap() ^= 1;
            }
            target.adopt_record(key, Some(&bytes)).unwrap();
        }
        assert!(target.finish_adoption().is_err());
    }
    state.validate().unwrap();
}

#[test]
fn mixed_bulk_revisions_are_repaired_by_final_deltas() {
    let mut scene = Scene::default();
    let mut session = Session::default();
    session.pending.create(1).unwrap();
    scene.sessions.insert(4, session);
    let mut source = Runtime::new(Channel(1), Some(Channel(2)), scene);
    let mut target = Runtime::<Scene>::empty();
    for key in source.keys() {
        let bytes = source.encode_record(key).unwrap().unwrap();
        target.adopt_record(key, Some(&bytes)).unwrap();
        if key >> 32 == 9 && key as u32 == 0 {
            source
                .component
                .sessions
                .get_mut(&4)
                .unwrap()
                .pending
                .node(1)
                .unwrap()
                .translation = (9, 7);
        }
    }
    target.prepare_adoption().unwrap();
    assert!(target.finish_adoption().is_err());
    for key in source.keys() {
        target
            .adopt_record(key, source.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    target.finish_adoption().unwrap();
    assert_eq!(
        target.component.sessions[&4].pending.nodes[&1].translation,
        (9, 7)
    );
}

#[test]
fn input_records_preserve_endpoints_focus_capture_and_pending_delivery() {
    use bexos_flatland_input::{
        Event, Key, Phase, Pointer,
        router::Capture,
        virtio::{Device, RawEvent},
    };
    use bexos_scened::input::Link;
    let mut scene = Scene::default();
    let mut session = Session::default();
    session.pending.create(1).unwrap();
    session.pending.root = Some(1);
    session.present(1).unwrap();
    scene.sessions.insert(4, session);
    let input = &mut scene.input;
    let mut device = Device::new(8);
    let key = device
        .feed_at(
            RawEvent {
                kind: 1,
                code: 30,
                value: 1,
            },
            800.,
            600.,
            10,
        )
        .unwrap();
    let Event::Key(key) = key else { panic!() };
    input.devices.insert(
        8,
        Link {
            subscription_deadline: None,
            control: Channel(8),
            reports: Channel(9),
            expected: Some(41),
            device,
        },
    );
    input.router.focus(Some(4), |_| {});
    input.router.key(key).unwrap();
    let pointer = Pointer {
        device: 8,
        id: 3,
        x: 40.,
        y: 90.,
        phase: Phase::Down,
        ..Default::default()
    };
    input.router.captures.insert(
        (8, 3),
        Capture {
            view: 4,
            node: 1,
            last: pointer,
        },
    );
    input.queues.entry(4).or_default().push(Event::Key(key));
    input
        .queues
        .entry(4)
        .or_default()
        .push(Event::Pointer(pointer));
    let source = Runtime::new(Channel(1), Some(Channel(2)), scene);
    let mut target = Runtime::<Scene>::empty();
    for key in source.keys() {
        target
            .adopt_record(key, source.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    target.finish_adoption().unwrap();
    assert_eq!(
        target.component.input.encode(),
        source.component.input.encode()
    );
    let input = &mut target.component.input;
    assert_eq!(input.devices[&8].reports.0, 9);
    assert_eq!(input.devices[&8].expected, Some(41));
    assert_eq!(input.router.focused, Some(4));
    assert_eq!(input.router.captures[&(8, 3)].last, pointer);
    assert_eq!(
        input.queues.get_mut(&4).unwrap().pop(),
        Some(Event::Key(key))
    );
    assert_eq!(
        input.queues.get_mut(&4).unwrap().pop(),
        Some(Event::Pointer(pointer))
    );
    let mut canceled = Vec::new();
    input.router.reset_device(8, |e| canceled.push(e.event));
    assert!(canceled.contains(&Event::Pointer(Pointer {
        phase: Phase::Cancel,
        ..pointer
    })));
    assert!(canceled.contains(&Event::Key(Key {
        state: 0,
        unicode: 0,
        ..key
    })));
    assert!(input.router.captures.is_empty());
    assert!(input.router.keys.is_empty());
}

#[test]
fn failed_input_delivery_resets_stream_and_suppresses_held_repeat() {
    use bexos_flatland_input::{Event, Key};
    let mut input = bexos_scened::input::Input::default();
    input.router.focus(Some(4), |_| {});
    let press = Key {
        device: 8,
        code: 30,
        state: 1,
        ..Default::default()
    };
    input.router.key(press).unwrap();
    input.queues.entry(4).or_default().push(Event::Key(press));
    input.reset_view(4);
    assert_eq!(input.queues.get_mut(&4).unwrap().pop(), Some(Event::Reset));
    assert!(input.queues[&4].is_empty());
    assert!(input.router.key(Key { state: 2, ..press }).is_none());
    assert!(input.router.key(Key { state: 0, ..press }).is_none());
    assert!(input.router.key(press).is_some());
}

#[test]
fn gesture_takeover_cancels_capture_and_preserves_shell_stream_across_transfer() {
    use bexos_flatland_input::{Event, Phase, Pointer, gestures::EdgePolicy, router::Capture};
    use bexos_scened::input::Input;
    let mut input = Input::default();
    input.delivered = 100;
    input
        .configure_edges(
            9,
            EdgePolicy {
                edges: 1,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(input.configure_edges(10, EdgePolicy::default()).is_err());
    let p = Pointer {
        device: 1,
        id: 3,
        x: 12.,
        y: 50.,
        phase: Phase::Down,
        ..Default::default()
    };
    input.router.captures.insert(
        (1, 3),
        Capture {
            view: 4,
            node: 1,
            last: p,
        },
    );
    assert!(!input.arbitrate(p, 800., 600.));
    let mut input = Input::decode(&input.encode()).unwrap();
    input.delivered = 100;
    let moved = Pointer {
        x: 60.,
        phase: Phase::Move,
        ..p
    };
    assert!(input.arbitrate(moved, 800., 600.));
    assert_eq!(
        input.queues.get_mut(&4).unwrap().pop(),
        Some(Event::Pointer(Pointer {
            phase: Phase::Cancel,
            ..p
        }))
    );
    assert!(input.router.captures.is_empty());
    let mut input = Input::decode(&input.encode()).unwrap();
    assert_eq!(input.gesture_owner, 9);
    assert_eq!(input.shell_events.pop(), Some(Event::Pointer(p)));
    assert_eq!(input.shell_events.pop(), Some(Event::Pointer(moved)));
    let up = Pointer {
        phase: Phase::Up,
        ..moved
    };
    assert!(input.arbitrate(up, 800., 600.));
    assert_eq!(input.shell_events.pop(), Some(Event::Pointer(up)));
    input.remove_shell(9);
    assert_eq!(input.gesture_owner, 0);
    assert!(!input.arbitrate(p, 800., 600.));
    assert!(!input.arbitrate(moved, 800., 600.));
}

#[test]
fn slow_shell_receives_reset_without_orphaned_gesture_releases() {
    use bexos_flatland_input::{Event, Phase, Pointer, gestures::EdgePolicy};
    use bexos_scened::input::Input;
    let mut input = Input::default();
    input
        .configure_edges(
            9,
            EdgePolicy {
                edges: 1,
                ..Default::default()
            },
        )
        .unwrap();
    for id in 0..64 {
        let p = Pointer {
            device: 1,
            id,
            x: 12.,
            y: 50.,
            phase: Phase::Down,
            ..Default::default()
        };
        assert!(!input.arbitrate(p, 800., 600.));
        assert!(input.arbitrate(
            Pointer {
                x: 60.,
                phase: Phase::Move,
                ..p
            },
            800.,
            600.
        ));
    }
    assert_eq!(input.shell_events.len(), 128);
    let up = Pointer {
        device: 1,
        id: 0,
        x: 60.,
        y: 50.,
        phase: Phase::Up,
        ..Default::default()
    };
    assert!(input.arbitrate(up, 800., 600.));
    assert_eq!(input.shell_events.pop(), Some(Event::Reset));
    assert!(!input.arbitrate(Pointer { id: 1, ..up }, 800., 600.));
    assert!(input.shell_events.is_empty());
}

#[test]
fn timing_records_preserve_legacy_encodings_until_activation() {
    use bexos_graphics_runtime::migration::Component;
    use bexos_migration::codec::Encoder;
    for (outer, inner) in [(1, 1), (2, 1), (2, 2), (3, 3)] {
        let mut w = Encoder::new();
        if outer != 1 {
            w.word(outer);
            w.word(0);
            w.word(0);
            if outer == 3 {
                w.word(16_667);
            }
        }
        w.word(inner);
        w.word(0);
        let bytes = w.finish();
        let mut s = Scene::default();
        s.adopt_component_record(4, Some(&bytes)).unwrap();
        assert_eq!(s.encode_component_record(4).unwrap().unwrap(), bytes);
        assert_eq!(s.frame_period_us(), if outer == 3 { 16_667 } else { 8_333 });
        let global = s.encode_component_record(1).unwrap().unwrap();
        s.adopt_component_record(1, Some(&global)).unwrap();
        assert_eq!(s.encode_component_record(4).unwrap().unwrap(), bytes);
        s.activate();
        assert_eq!(
            s.frame_period_ns(),
            if outer == 3 { 16_667_000 } else { 8_333_000 }
        );
    }
}
#[test]
fn desktop_refresh_configuration_accepts_only_supported_profiles() {
    use bexos_scened::desktop_config::DesktopConfig;
    let bytes = [10, 1, b'a', 16, 24, 29, 255, 255, 255, 255];
    assert_eq!(
        DesktopConfig::decode(&bytes).unwrap().frame_period_us(),
        8_333
    );
    for (hz, expected) in [
        (0, Some(8_333)),
        (60, Some(16_667)),
        (120, Some(8_333)),
        (90, None),
    ] {
        let mut config = bytes.to_vec();
        config.extend([72, hz]);
        assert_eq!(
            DesktopConfig::decode(&config)
                .ok()
                .map(|c| c.frame_period_us()),
            expected
        );
    }
}
