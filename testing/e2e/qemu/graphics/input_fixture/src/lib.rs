use bexos_graphics_runtime::{
    self as rt, Mapping,
    migration::{Component, Runtime},
};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{
    Channel, Memory, Startup,
    live_migration::{Resource, Source},
};
use graphics_fidl::*;
mod accessibility;
mod allocation_probe;
mod condition;
mod disconnect;
pub mod performance;
mod styled;
mod synchronization;
mod transitions;
mod venus;
mod venus_query;
#[derive(Default)]
pub struct Fixture {
    pub performance: performance::Performance,
    pub transition: transitions::Transition,
    pub accessibility: accessibility::Accessibility,
    pub sessions: Vec<Channel>,
    pub buffers: Vec<Mapping>,
    pub received: u64,
    pub releases: [u64; 2],
    pub pending_acquire: u64,
    pub pending_release: u64,
    pub pending_sequence: u64,
}
impl Component for Fixture {
    fn encode(&self, w: &mut Encoder) -> Result<(), Error> {
        w.word(11);
        w.word(self.received);
        w.word(self.sessions.len() as u64);
        for c in &self.sessions {
            w.word(c.0)
        }
        w.word(self.buffers.len() as u64);
        for b in &self.buffers {
            b.encode(w)
        }
        for value in self.releases.into_iter().chain([
            self.pending_acquire,
            self.pending_release,
            self.pending_sequence,
        ]) {
            w.word(value);
        }
        self.accessibility.encode(w);
        self.transition.encode(w);
        self.performance.encode(w);
        Ok(())
    }
    fn decode(&mut self, r: &mut Decoder<'_>) -> Result<(), Error> {
        let version = r.word()?;
        if !(1..=11).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        self.received = r.word()?;
        self.sessions.clear();
        for _ in 0..r.count(2)? {
            self.sessions.push(Channel(r.word()?))
        }
        self.buffers.clear();
        for _ in 0..r.count(if version >= 9 {
            4
        } else if version >= 8 {
            3
        } else {
            2
        })? {
            self.buffers.push(Mapping::decode(r)?);
        }
        if version >= 2 {
            self.releases = [r.word()?, r.word()?];
            self.pending_acquire = r.word()?;
            self.pending_release = r.word()?;
            self.pending_sequence = r.word()?;
        }
        if version >= 3 {
            self.accessibility = accessibility::Accessibility::decode(r, version)?;
        }
        if version >= 9 {
            self.transition = transitions::Transition::decode(r)?;
            if version == 9 && self.transition.phase > 4 {
                return Err(Error::InvalidData);
            }
        }
        if version >= 11 {
            self.performance = performance::Performance::decode(r)?;
        }
        self.validate()
    }
    fn validate(&self) -> Result<(), Error> {
        if self.sessions.len() != 2
            || !(2..=4).contains(&self.buffers.len())
            || self.sessions[0].0 == 0
            || (self.sessions[1].0 == 0) != (self.transition.phase >= 5)
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
    fn resources(&self) -> Vec<Resource> {
        let mut out: Vec<_> = self
            .sessions
            .iter()
            .filter(|c| c.0 != 0)
            .map(|c| Resource::Handle(c.0))
            .collect();
        out.extend(self.accessibility.resources());
        if self.transition.release != 0 {
            out.push(Resource::Handle(self.transition.release));
        }
        out.extend(self.performance.resources());
        for b in &self.buffers {
            out.extend(b.resources())
        }
        out.extend(
            self.releases
                .into_iter()
                .chain([self.pending_acquire, self.pending_release])
                .filter(|h| *h != 0)
                .map(Resource::Handle),
        );
        out
    }
    fn activate(&mut self) {
        self.performance.activate();
        if let Some(probe) = &mut self.accessibility.venus {
            probe.mapping.owned = true;
        }
        for b in &mut self.buffers {
            b.owned = true;
        }
    }
}
fn call<Q: FidlEncode>(c: &mut Channel, ordinal: u64, q: &Q) {
    let r: FlatlandSessionPresentResponse = rt::call(c, ordinal, q).expect("fixture request");
    assert_eq!(r.status, Status::Ok);
}
fn fenced(c: &mut Channel) -> (u64, Channel, Channel) {
    let (signal, acquire) = Channel::pair().unwrap();
    let (release, notify) = Channel::pair().unwrap();
    let response: FlatlandSessionPresentWithFencesResponse = rt::call(
        c,
        16,
        &FlatlandSessionPresentWithFencesRequest {
            presentation_time_ticks: 0,
            acquire: HandleRef { raw: acquire.0 },
            release: HandleRef { raw: notify.0 },
        },
    )
    .unwrap();
    assert_eq!(response.status, Status::Ok);
    (response.sequence, signal, release)
}
pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).expect("fixture startup");
    bexos_userspace::log("input-fixture: startup received\n");
    let mut runtime = if start.migration_target {
        bexos_userspace::live_migration::receive::<Runtime<Fixture>>(
            control,
            start.migration_generation,
        )
        .unwrap()
    } else {
        synchronization::verify();
        condition::verify();
        let mut fixture = Fixture::default();
        fixture.accessibility.display = start
            .service_grants
            .iter()
            .find(|g| g.protocol == "DisplayCoordinator" && g.capability == "Public")
            .unwrap()
            .endpoint;
        fixture.accessibility.gpu = start
            .service_grants
            .iter()
            .find(|g| g.protocol == "DisplayCoordinator" && g.capability == "GpuTransport")
            .unwrap()
            .endpoint;
        fixture.accessibility.control = start
            .service_grants
            .iter()
            .find(|g| g.protocol == "AccessibilityControl")
            .unwrap()
            .endpoint;
        fixture.accessibility.shell = start
            .service_grants
            .iter()
            .find(|g| g.protocol == "ShellControl")
            .unwrap()
            .endpoint;
        for (index, grant) in start
            .service_grants
            .iter()
            .filter(|g| g.protocol == "FlatlandSession")
            .enumerate()
        {
            let mut c = Channel(grant.endpoint);
            call(
                &mut c,
                1,
                &FlatlandSessionCreateTransformRequest { node_id: 1 },
            );
            call(&mut c, 2, &FlatlandSessionSetRootRequest { node_id: 1 });
            call(
                &mut c,
                4,
                &FlatlandSessionSetTranslationRequest {
                    node_id: 1,
                    x: index as i32 * 400,
                    y: 0,
                },
            );
            let mut buffer = Mapping::new(400 * 600 * 4).unwrap();
            for p in buffer.bytes_mut().chunks_exact_mut(4) {
                p.copy_from_slice(if index == 0 {
                    &[150, 60, 30, 255]
                } else {
                    &[30, 80, 160, 255]
                });
            }
            call(
                &mut c,
                8,
                &FlatlandSessionSetContentRequest {
                    node_id: 1,
                    buffer: HandleRef {
                        raw: Memory::duplicate(buffer.handle, 1 | 2 | 16 | 32).unwrap(),
                    },
                    surface: Surface {
                        width: 400,
                        height: 600,
                        stride: 1600,
                        format: 1,
                    },
                },
            );
            let reference: FlatlandSessionGetViewReferenceResponse =
                rt::call(&mut c, 17, &FlatlandSessionGetViewReferenceRequest {}).unwrap();
            assert_eq!(reference.status, Status::Ok);
            let reference = reference.view.unwrap();
            fixture.accessibility.tokens[index] = reference.token.raw;
            fixture.accessibility.identities[index] = reference.view_id;
            fixture.sessions.push(c);
            fixture.buffers.push(buffer);
        }
        assert_eq!(fixture.sessions.len(), 2);
        styled::configure(&mut fixture);
        let (fake, peer) = Channel::pair().unwrap();
        Memory::close(peer.0).unwrap();
        let rejected: FlatlandSessionSetViewContentResponse = rt::call(
            &mut fixture.sessions[0],
            18,
            &FlatlandSessionSetViewContentRequest {
                node_id: 1,
                token: HandleRef { raw: fake.0 },
            },
        )
        .unwrap();
        assert_eq!(rejected.status, Status::ErrInvalidArgs);
        call(
            &mut fixture.sessions[0],
            18,
            &FlatlandSessionSetViewContentRequest {
                node_id: 1,
                token: HandleRef {
                    raw: Memory::duplicate(fixture.accessibility.tokens[1], 1 | 32).unwrap(),
                },
            },
        );
        call(
            &mut fixture.sessions[0],
            4,
            &FlatlandSessionSetTranslationRequest {
                node_id: 1,
                x: 10,
                y: 0,
            },
        );
        for (index, c) in fixture.sessions.iter_mut().enumerate() {
            let (sequence, acquire, release) = fenced(c);
            assert_eq!(sequence, 1);
            acquire.send(&[], &[]).unwrap();
            Memory::close(acquire.0).unwrap();
            fixture.releases[index] = release.0;
        }
        // Complete offscreen backend verification before advertising readiness.
        // Vello's pinned workspace sizes are substantial even for tiny scenes;
        // the fixture and compositor must not initialize two full workspaces in
        // the QEMU device's shared 256 MiB host-visible aperture at once.
        fixture.accessibility.verify_gpu();
        Startup::ready(control).unwrap();
        bexos_userspace::log("input-fixture: two persistent views ready\n");
        Runtime::new(control, start.migration, fixture)
    };
    let mut source = Source::new(runtime.migration);
    loop {
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        runtime.component.accessibility.setup();
        runtime.component.accessibility.poll_gestures();
        let mut workload = 0;
        let mut toggle = false;
        let mut disconnect = false;
        for (index, c) in runtime.component.sessions.iter_mut().enumerate() {
            if c.0 == 0 {
                continue;
            }
            // Responses borrow IPC storage, so decode immediately instead of returning
            // a borrowed vector through the generic owned-response RPC helper.
            let mut out = [0; 32];
            out[..8].copy_from_slice(&15u64.to_le_bytes());
            let n = FlatlandSessionReadInputRequest {}
                .encode(&mut out[8..], &mut [])
                .unwrap();
            c.send(&out[..8 + n.bytes], &[]).unwrap();
            let m = c.recv_with_timeout(2).unwrap();
            let r = FlatlandSessionReadInputResponse::decode(&m.bytes, &[]).unwrap();
            assert_eq!(r.status, Status::Ok);
            for i in 0..r.events.len() {
                let e = r.events.get(i).unwrap();
                if e.kind == 2 && (59..=63).contains(&e.code) && e.key_state == 0 {
                    workload = e.code - 58;
                }
                toggle |= e.kind == 2 && e.code == 68 && e.key_state == 0;
                disconnect |= e.kind == 2 && e.code == 66 && e.key_state == 0;
                if index == 0 && e.kind == 2 && e.code == 88 && e.key_state == 0 {
                    runtime.component.accessibility.acknowledge_visual();
                }
                if index == 0 && e.kind == 2 && e.code == 30 {
                    if e.key_state == 1 && runtime.component.pending_sequence == 0 {
                        let (sequence, acquire, release) = fenced(c);
                        runtime.component.pending_sequence = sequence;
                        runtime.component.pending_acquire = acquire.0;
                        runtime.component.pending_release = release.0;
                        bexos_userspace::log("input-fixture: acquire held across transplant\n");
                    } else if e.key_state == 0 && runtime.component.pending_acquire != 0 {
                        runtime.component.accessibility.transplanted();
                        let response: FlatlandSessionGetPresentationInfoResponse =
                            rt::call(c, 14, &FlatlandSessionGetPresentationInfoRequest {}).unwrap();
                        assert_eq!(
                            response.pending_count, 1,
                            "unsignaled acquire latched early"
                        );
                        let acquire = runtime.component.pending_acquire;
                        Channel(acquire).send(&[], &[]).unwrap();
                        Memory::close(acquire).unwrap();
                        runtime.component.pending_acquire = 0;
                    }
                }
                runtime.component.received += 1;
                if e.kind != 2 || e.key_state != 2 || runtime.component.received <= 16 {
                    bexos_userspace::log(&format!(
                        "input-fixture: view={index} kind={} phase={} code={} state={} x={} y={} id={} device={}\n",
                        e.kind, e.phase, e.code, e.key_state, e.x, e.y, e.id, e.device
                    ));
                }
            }
        }
        if runtime.component.pending_sequence != 0 && runtime.component.releases[0] != 0 {
            if let Ok(message) = Channel(runtime.component.releases[0]).try_recv() {
                assert_eq!(
                    runtime.component.pending_acquire, 0,
                    "buffers released before acquire"
                );
                let release = PresentationRelease::decode(&message.bytes, &[]).unwrap();
                assert_eq!(release.status, Status::Ok);
                assert_eq!(release.sequence, 1);
                Memory::close(runtime.component.releases[0]).unwrap();
                runtime.component.releases[0] = 0;
                bexos_userspace::log("input-fixture: retired buffer lease after transplant\n");
            }
        }
        if toggle {
            transitions::toggle(&mut runtime.component);
        }
        transitions::poll(&mut runtime.component);
        if disconnect {
            disconnect::start(&mut runtime.component);
        }
        disconnect::poll(&mut runtime.component);
        if workload != 0 {
            performance::start(&mut runtime.component, workload);
        }
        performance::poll(&mut runtime.component);
        source.changed(1);
        rt::wait(&[control], rt::now_us().saturating_add(5000));
    }
}
