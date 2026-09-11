//! Privileged fixture checks actual token authority, geometry and visual controls.
use bexos_graphics_runtime as rt;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, Memory, live_migration::Resource};
use graphics_fidl::*;
#[derive(Default)]
pub struct Accessibility {
    pub control: u64,
    pub shell: u64,
    pub tokens: [u64; 2],
    pub identities: [u64; 2],
    pub checked: bool,
    pub ready: bool,
    pub generations: [u64; 2],
    pub gesture_phase: u64,
    pub display: u64,
    pub gpu_fingerprint: u64,
    pub gpu: u64,
    pub venus: Option<crate::venus::Probe>,
}
impl Accessibility {
    pub fn encode(&self, w: &mut Encoder) {
        for v in [
            self.control,
            self.shell,
            self.tokens[0],
            self.tokens[1],
            self.identities[0],
            self.identities[1],
            self.checked as u64,
            self.ready as u64,
            self.generations[0],
            self.generations[1],
            self.gesture_phase,
            self.display,
            self.gpu_fingerprint,
            self.gpu,
        ] {
            w.word(v);
        }
        w.word(self.venus.is_some() as u64);
        if let Some(probe) = &self.venus {
            probe.encode(w);
        }
    }
    pub fn decode(r: &mut Decoder<'_>, version: u64) -> Result<Self, Error> {
        Ok(Self {
            control: r.word()?,
            shell: r.word()?,
            tokens: [r.word()?, r.word()?],
            identities: [r.word()?, r.word()?],
            checked: r.flag()?,
            ready: r.flag()?,
            generations: [r.word()?, r.word()?],
            gesture_phase: if version >= 4 { r.word()? } else { 0 },
            display: if version >= 5 { r.word()? } else { 0 },
            gpu_fingerprint: if version >= 5 { r.word()? } else { 0 },
            gpu: if version >= 6 { r.word()? } else { 0 },
            venus: if version >= 7 && r.flag()? {
                Some(crate::venus::Probe::decode(r)?)
            } else {
                None
            },
        })
    }
    pub fn resources(&self) -> impl Iterator<Item = Resource> {
        [
            self.control,
            self.shell,
            self.tokens[0],
            self.tokens[1],
            self.display,
            self.gpu,
        ]
        .into_iter()
        .filter(|h| *h != 0)
        .map(Resource::Handle)
        .chain(self.venus.iter().flat_map(|p| p.mapping.resources()))
    }
    fn token(&self, index: usize) -> HandleRef {
        HandleRef {
            raw: Memory::duplicate(self.tokens[index], 1 | 32).unwrap(),
        }
    }
    pub fn geometry(&self, index: usize) -> ViewGeometry {
        let r: AccessibilityControlGetViewGeometryResponse = rt::call(
            &mut Channel(self.control),
            1,
            &AccessibilityControlGetViewGeometryRequest {
                token: self.token(index),
            },
        )
        .unwrap();
        assert_eq!(r.status, Status::Ok);
        assert_eq!(r.geometry.view_id, self.identities[index]);
        r.geometry
    }
    fn wait_scale(&self, scale: f64) -> ViewGeometry {
        let deadline = rt::now_us() + 2_000_000;
        loop {
            let geometry = self.geometry(0);
            if geometry.scale_x == scale {
                return geometry;
            }
            assert!(
                rt::now_us() < deadline,
                "display transform missed frame boundary"
            );
            rt::wait(&[], rt::now_us() + 1_000);
        }
    }
    pub fn setup(&mut self) {
        if self.checked {
            return;
        }
        let left = self.geometry(0);
        let right = self.geometry(1);
        if !left.visible || !right.visible {
            return;
        }
        assert_eq!(
            (left.x, left.y, left.scale_x, left.scale_y),
            (10., 0., 1., 1.)
        );
        assert_eq!((right.x, right.y), (410., 0.));
        assert_eq!((left.view_generation, right.view_generation), (1, 1));
        self.generations = [left.view_generation, right.view_generation];
        let (fake, peer) = Channel::pair().unwrap();
        Memory::close(peer.0).unwrap();
        let rejected: AccessibilityControlGetViewGeometryResponse = rt::call(
            &mut Channel(self.control),
            1,
            &AccessibilityControlGetViewGeometryRequest {
                token: HandleRef { raw: fake.0 },
            },
        )
        .unwrap();
        assert_eq!(rejected.status, Status::ErrAccessDenied);
        let bounds = Rect {
            x: 150.,
            y: 100.,
            width: 50.,
            height: 40.,
        };
        let stale: AccessibilityControlSetFocusRingResponse = rt::call(
            &mut Channel(self.control),
            2,
            &AccessibilityControlSetFocusRingRequest {
                token: self.token(0),
                view_generation: left.view_generation + 1,
                bounds,
                width: 2,
                rgba: 0xff0000ff,
            },
        )
        .unwrap();
        assert_eq!(stale.status, Status::ErrInvalidArgs);
        let ring: AccessibilityControlSetFocusRingResponse = rt::call(
            &mut Channel(self.control),
            2,
            &AccessibilityControlSetFocusRingRequest {
                token: self.token(0),
                view_generation: left.view_generation,
                bounds,
                width: 2,
                rgba: 0xff0000ff,
            },
        )
        .unwrap();
        assert_eq!(ring.status, Status::Ok);
        let focus: ShellControlFocusViewResponse = rt::call(
            &mut Channel(self.shell),
            1,
            &ShellControlFocusViewRequest {
                token: self.token(0),
            },
        )
        .unwrap();
        assert_eq!(focus.status, Status::Ok);
        let raise: ShellControlRaiseViewResponse = rt::call(
            &mut Channel(self.shell),
            2,
            &ShellControlRaiseViewRequest {
                token: self.token(0),
            },
        )
        .unwrap();
        assert_eq!(raise.status, Status::Ok);
        let display: AccessibilityControlSetDisplayTransformResponse = rt::call(
            &mut Channel(self.control),
            4,
            &AccessibilityControlSetDisplayTransformRequest {
                scale: 2.,
                origin_x: 100.,
                origin_y: 50.,
                color_filter: 1,
            },
        )
        .unwrap();
        assert_eq!(display.status, Status::Ok);
        let left = self.wait_scale(2.);
        assert_eq!((left.x, left.y, left.scale_x), (-180., -100., 2.));
        self.checked = true;
        bexos_userspace::log("input-fixture: accessibility magnification active\n");
    }
    pub fn acknowledge_visual(&mut self) {
        if !self.checked || self.ready {
            return;
        }
        let display: AccessibilityControlSetDisplayTransformResponse = rt::call(
            &mut Channel(self.control),
            4,
            &AccessibilityControlSetDisplayTransformRequest {
                scale: 1.,
                origin_x: 0.,
                origin_y: 0.,
                color_filter: 0,
            },
        )
        .unwrap();
        assert_eq!(display.status, Status::Ok);
        self.wait_scale(1.);
        let edges: ShellControlConfigureReservedEdgesResponse = rt::call(
            &mut Channel(self.shell),
            3,
            &ShellControlConfigureReservedEdgesRequest {
                edges: 1,
                inset: 16.,
                threshold: 32.,
            },
        )
        .unwrap();
        assert_eq!(edges.status, Status::Ok);
        self.ready = true;
        bexos_userspace::log("input-fixture: accessibility controls verified\n");
    }
    pub fn poll_gestures(&mut self) {
        if !self.ready {
            return;
        }
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&4u64.to_le_bytes());
        let n = ShellControlReadGesturesRequest {}
            .encode(&mut bytes[8..], &mut [])
            .unwrap();
        let channel = Channel(self.shell);
        channel.send(&bytes[..8 + n.bytes], &[]).unwrap();
        let message = channel.recv_with_timeout(2).unwrap();
        let response = ShellControlReadGesturesResponse::decode(&message.bytes, &[]).unwrap();
        assert_eq!(response.status, Status::Ok);
        for i in 0..response.events.len() {
            let e = response.events.get(i).unwrap();
            assert_eq!((e.kind, e.id), (1, 4));
            match e.phase {
                1 => {
                    assert_eq!(self.gesture_phase, 0);
                    assert!(e.x < 16.);
                    self.gesture_phase = 1;
                }
                2 => {
                    assert!((1..=2).contains(&self.gesture_phase));
                    assert!(e.x > 40.);
                    self.gesture_phase = 2;
                }
                3 => {
                    assert_eq!(self.gesture_phase, 2);
                    self.gesture_phase = 3;
                }
                _ => panic!("unexpected gesture cancellation/reset"),
            }
            bexos_userspace::log(&format!("input-fixture: shell gesture phase={}\n", e.phase));
        }
    }
    pub fn transplanted(&mut self) {
        self.verify_gpu();
        let left = self.geometry(0);
        let right = self.geometry(1);
        assert_eq!(
            (left.view_generation, right.view_generation),
            (self.generations[0], self.generations[1])
        );
        assert!(left.visible && right.visible);
        assert_eq!((left.x, left.y, left.scale_x), (10., 0., 1.));
        bexos_userspace::log("input-fixture: accessibility tokens survived transplant\n");
    }
    pub(crate) fn verify_gpu(&mut self) {
        let mut channel = Channel(self.display);
        let caps = rt::discovery::read(&mut channel).unwrap();
        assert!(
            caps.gpu
                .modes
                .iter()
                .any(|m| m.enabled && m.width > 0 && m.height > 0)
        );
        assert_eq!(
            caps.gpu.negotiated,
            bexos_virtio_gpu_protocol::transport::negotiated_features(caps.gpu.offered)
        );
        assert!(!caps.hardware_vsync && caps.direct_scanout);
        assert!(rt::discovery::capset(&mut channel, u32::MAX, 0).is_err());
        if bexos_virtio_gpu_protocol::transport::supported(&caps.gpu) {
            if self.gpu_fingerprint == 0 {
                #[cfg(bexos_guest)]
                {
                    let version = caps.gpu.capsets.iter().find(|c| c.id == 4).unwrap().version;
                    let capset = rt::discovery::capset(&mut channel, 4, version).unwrap();
                    bexos_userspace::log("input-fixture: initializing linked Mesa Venus Vulkan\n");
                    assert!(bexos_venus_transport::install(Channel(self.gpu), capset).is_ok());
                    bexos_venus_transport::probe::verify_submission()
                        .expect("Mesa Vulkan queue submission and readback");
                    bexos_userspace::log("input-fixture: initializing Vello through wgpu/Venus\n");
                    bexos_userspace::block_on(bexos_venus_wgpu::probe::verify_rendering())
                        .expect("Vello Vulkan rendering and texture readback");
                    bexos_userspace::log(
                        "input-fixture: Vello Vulkan rendering/readback verified\n",
                    );
                    self.gpu = bexos_venus_transport::uninstall()
                        .expect("Mesa releases all transport leases")
                        .0;
                    bexos_userspace::log(
                        "input-fixture: linked Mesa Vulkan queue/fence/readback verified\n",
                    );
                    bexos_venus_wgpu::worker_probe::verify(Channel(self.gpu))
                        .expect("GPU worker repair/failure/restart");
                }
                self.venus = Some(crate::venus::verify(&mut Channel(self.gpu)));
            } else if let Some(mut probe) = self.venus.take() {
                probe.verify(&mut Channel(self.gpu));
                probe.release(&mut Channel(self.gpu));
                bexos_userspace::log(
                    "input-fixture: Venus context/mapping/fence survived transplant\n",
                );
            }
        }
        let mut w = Encoder::new();
        caps.gpu.encode(&mut w);
        let hash = bexos_migration::codec::checksum(&w.finish());
        if self.gpu_fingerprint == 0 {
            self.gpu_fingerprint = hash;
        } else {
            assert_eq!(self.gpu_fingerprint, hash);
        }
        bexos_userspace::log("input-fixture: GPU capabilities verified\n");
    }
}
