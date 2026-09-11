//! QMP F10 switches a persistent client into direct scanout and back. Each
//! phase and its release endpoint are logical state across live replacement.
use bexos_graphics::{Format, Surface};
use bexos_graphics_runtime::{self as rt, Mapping, flatland::Session};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, Memory};
use graphics_fidl::*;
#[derive(Default)]
pub struct Transition {
    pub phase: u64,
    pub sequence: u64,
    pub release: u64,
    // Diagnostic deadline restarts after process replacement; it does not alter
    // the logical transaction or its migrated acquire/release ownership.
    deadline_us: u64,
    last_progress: Option<(u64, u64, u64)>,
}
impl Transition {
    pub fn encode(&self, w: &mut Encoder) {
        w.word(self.phase);
        w.word(self.sequence);
        w.word(self.release);
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        let s = Self {
            phase: r.word()?,
            sequence: r.word()?,
            release: r.word()?,
            deadline_us: 0,
            last_progress: None,
        };
        if s.phase > 6
            || (s.phase == 0) != (s.sequence == 0)
            || (s.phase == 0 || s.phase >= 4) != (s.release == 0)
        {
            return Err(Error::InvalidData);
        }
        Ok(s)
    }
}
pub fn toggle(f: &mut crate::Fixture) {
    assert!(
        f.accessibility.ready,
        "transition before accessibility setup"
    );
    assert_eq!(
        f.pending_acquire, 0,
        "transition before held acquire release"
    );
    bexos_userspace::log(&format!(
        "input-fixture: transition requested phase={}\n",
        f.transition.phase
    ));
    let mut client = Session::new(f.sessions[0]);
    match f.transition.phase {
        0 => {
            let clear: AccessibilityControlClearFocusRingResponse = rt::call(
                &mut Channel(f.accessibility.control),
                3,
                &AccessibilityControlClearFocusRingRequest {},
            )
            .unwrap();
            assert_eq!(clear.status, Status::Ok);
            let focus: ShellControlFocusViewResponse = rt::call(
                &mut Channel(f.accessibility.shell),
                1,
                &ShellControlFocusViewRequest {
                    token: HandleRef {
                        raw: Memory::duplicate(f.accessibility.tokens[0], 1 | 32).unwrap(),
                    },
                },
            )
            .unwrap();
            assert_eq!(focus.status, Status::Ok);
            let surface = Surface {
                width: 800,
                height: 600,
                stride: 3200,
                format: Format::Bgrx,
            };
            let mut buffer = Mapping::new(800 * 600 * 4).unwrap();
            for pixel in buffer.bytes_mut().chunks_exact_mut(4) {
                // X deliberately differs from alpha: scanout must use format,
                // never infer opacity from pixel values.
                pixel.copy_from_slice(&[41, 83, 167, 0]);
            }
            client.create(3).unwrap();
            client.content(3, buffer.handle, surface).unwrap();
            client.root(3).unwrap();
            f.buffers.push(buffer);
            let (sequence, acquire, release) = crate::fenced(&mut client.channel);
            acquire.send(&[], &[]).unwrap();
            Memory::close(acquire.0).unwrap();
            f.transition = Transition {
                phase: 1,
                sequence,
                release: release.0,
                deadline_us: rt::now_us().saturating_add(30_000_000),
                last_progress: None,
            };
            bexos_userspace::log(&format!(
                "input-fixture: fullscreen accepted sequence={sequence}\n"
            ));
        }
        2 => {
            client.root(1).unwrap();
            client.remove(3).unwrap();
            // Legacy Present is sufficient here: original buffers remain
            // immutable. The preceding direct lease must retire on completion.
            client.present(0).unwrap();
            f.transition.sequence = client.presentation_info().unwrap().accepted_sequence;
            f.transition.phase = 3;
            f.transition.deadline_us = rt::now_us().saturating_add(30_000_000);
            f.transition.last_progress = None;
        }
        _ => {}
    }
}
pub fn poll(f: &mut crate::Fixture) {
    if ![1, 3].contains(&f.transition.phase) {
        return;
    }
    let info = Session::new(f.sessions[0]).feedback().unwrap();
    assert_eq!(info.status, Status::Ok);
    let progress = (
        info.latched_sequence,
        info.presented_sequence,
        info.rejected_sequence,
    );
    if f.transition.last_progress != Some(progress) {
        bexos_userspace::log(&format!(
            "input-fixture: transition progress phase={} sequence={} latched={} presented={} rejected={}\n",
            f.transition.phase, f.transition.sequence, progress.0, progress.1, progress.2
        ));
        f.transition.last_progress = Some(progress);
    }
    assert!(
        info.rejected_sequence < f.transition.sequence,
        "transition rejected: {info:?}"
    );
    if f.transition.deadline_us == 0 {
        f.transition.deadline_us = rt::now_us().saturating_add(30_000_000);
    }
    assert!(
        rt::now_us() < f.transition.deadline_us,
        "transition phase={} stalled: {info:?}",
        f.transition.phase
    );
    if info.presented_sequence < f.transition.sequence {
        return;
    }
    if f.transition.phase == 1 {
        assert!(
            matches!(Channel(f.transition.release).try_recv(), Err(error) if error == bexos_userspace::ipc::status(-6)),
            "scanout lease released or closed while visible"
        );
        f.transition.phase = 2;
        bexos_userspace::log("input-fixture: fullscreen opaque presentation complete\n");
    } else {
        let Ok(message) = Channel(f.transition.release).try_recv() else {
            return;
        };
        assert!(message.handles.is_empty());
        let released = PresentationRelease::decode(&message.bytes, &[]).unwrap();
        assert_eq!(released.status, Status::Ok);
        assert_eq!(released.sequence + 1, f.transition.sequence);
        Memory::close(f.transition.release).unwrap();
        f.transition.release = 0;
        // Keep the completed sequence as a marker; no endpoint remains owned.
        f.transition.phase = 4;
        f.buffers.pop();
        bexos_userspace::log("input-fixture: composition restored; scanout lease retired\n");
    }
}
