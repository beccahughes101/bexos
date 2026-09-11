//! Capability-backed view references and privately granted compositor controls.
//! Semantic trees stay with the future accessibility broker.
use crate::state::Scene;
use bexos_graphics::{
    accessibility::{DisplayTransform, valid_rect},
    resolved::{Rect, Transform},
};
use bexos_graphics_runtime as rt;
use bexos_userspace::{Channel, Memory, service_binding::BoundServiceEndpoint};
use graphics_fidl::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug)]
pub struct View {
    pub token: u64,
    pub identity: u64,
    pub generation: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct FocusRing {
    pub view: u64,
    pub generation: u64,
    pub bounds: Rect,
    pub width: u32,
    pub rgba: u32,
}
#[derive(Default)]
pub struct Controls {
    pub views: BTreeMap<u64, View>,
    pub committed: BTreeMap<u64, u64>,
    pub embedded: std::collections::BTreeSet<u64>,
    pub accessibility: Vec<BoundServiceEndpoint>,
    pub shell: Vec<BoundServiceEndpoint>,
    pub stacking: Vec<u64>,
    pub generation: u64,
    pub ring: Option<FocusRing>,
    pub display: DisplayTransform,
    pub pending_display: Option<DisplayTransform>,
    pub pending_ring: Option<Option<FocusRing>>,
    pub pending_stacking: Option<Vec<u64>>,
    pub scratch: Option<rt::Mapping>,
}
impl Controls {
    pub fn lookup_identity(&self, identity: u64) -> Result<u64, Status> {
        self.views
            .iter()
            .find_map(|(owner, v)| (v.identity == identity).then_some(*owner))
            .ok_or(Status::ErrAccessDenied)
    }
    pub(crate) fn lookup(&self, token: u64) -> Result<u64, Status> {
        let (identity, _) = Memory::channel_identity(token).map_err(|_| Status::ErrAccessDenied)?;
        self.lookup_identity(identity)
    }
    pub fn disconnected(&mut self, owner: u64) {
        if let Some(view) = self.views.remove(&owner) {
            let _ = Memory::close(view.token);
        }
        self.stacking.retain(|id| *id != owner);
        if let Some(stacking) = &mut self.pending_stacking {
            stacking.retain(|id| *id != owner);
        }
        self.committed.remove(&owner);
        self.embedded.remove(&owner);
        if self.ring.is_some_and(|r| r.view == owner) {
            self.ring = None;
        }
        if self
            .pending_ring
            .is_some_and(|r| r.is_some_and(|r| r.view == owner))
        {
            self.pending_ring = Some(None);
        }
        self.changed();
    }
    pub fn changed(&mut self) {
        self.generation = self.generation.saturating_add(1);
    }
    pub fn committed(&mut self, owner: u64, sequence: u64) {
        self.committed.insert(owner, sequence);
        if let Some(view) = self.views.get_mut(&owner) {
            view.generation = sequence;
        }
    }
    pub fn register(&mut self, owner: u64) {
        if !self.stacking.contains(&owner) && self.stacking.len() < 16 {
            self.stacking.push(owner);
        }
        if let Some(stacking) = &mut self.pending_stacking {
            if !stacking.contains(&owner) && stacking.len() < 16 {
                stacking.push(owner);
            }
        }
    }
    pub fn latch(&mut self) -> bool {
        let changed = self.pending_display.is_some()
            || self.pending_ring.is_some()
            || self.pending_stacking.is_some();
        if let Some(display) = self.pending_display.take() {
            self.display = display;
        }
        if let Some(ring) = self.pending_ring.take() {
            self.ring = ring;
        }
        if let Some(stacking) = self.pending_stacking.take() {
            self.stacking = stacking;
        }
        if changed {
            self.changed();
        }
        changed
    }
}
pub fn reference(s: &mut Scene, channel: Channel, bytes: &[u8], handles: &[u64]) {
    let result = (|| {
        if !handles.is_empty()
            || FlatlandSessionGetViewReferenceRequest::decode(bytes, &[]).is_err()
        {
            return Err(Status::ErrInvalidArgs);
        }
        if !s.sessions.contains_key(&channel.0) && s.sessions.len() >= 16 {
            return Err(Status::ErrNoMemory);
        }
        s.sessions.entry(channel.0).or_default();
        if !s.controls.views.contains_key(&channel.0) {
            let (token, peer) = Channel::pair().map_err(|_| Status::ErrNoMemory)?;
            let _ = Memory::close(peer.0);
            let identity = match Memory::channel_identity(token.0) {
                Ok((identity, _)) => identity,
                Err(_) => {
                    let _ = Memory::close(token.0);
                    return Err(Status::ErrIo);
                }
            };
            let generation = s.controls.committed.get(&channel.0).copied().unwrap_or(0);
            s.controls.views.insert(
                channel.0,
                View {
                    token: token.0,
                    identity,
                    generation,
                },
            );
        }
        let view = &s.controls.views[&channel.0];
        let token = Memory::duplicate(view.token, 1 | 32).map_err(|_| Status::ErrNoMemory)?;
        Ok(ViewReference {
            token: HandleRef { raw: token },
            view_id: view.identity,
        })
    })();
    let status = result.as_ref().map_or_else(|s| *s, |_| Status::Ok);
    rt::reply(
        channel,
        &FlatlandSessionGetViewReferenceResponse {
            status,
            view: result.ok(),
        },
    );
}
fn rect(r: graphics_fidl::Rect) -> Rect {
    Rect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}
fn wire(r: Rect) -> graphics_fidl::Rect {
    graphics_fidl::Rect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}
pub fn geometry(s: &Scene, owner: u64) -> Result<ViewGeometry, Status> {
    let view = s
        .controls
        .views
        .get(&owner)
        .ok_or(Status::ErrAccessDenied)?;
    let session = s.sessions.get(&owner).ok_or(Status::ErrAccessDenied)?;
    let geometry = session
        .committed
        .root
        .and_then(|root| s.snapshots.get(&owner)?.geometry(root));
    let (transform, clip, visible) =
        geometry.unwrap_or((Transform::default(), Rect::default(), false));
    let transform = s.controls.display.world(transform);
    let target = s.canvas.as_ref().map_or(Rect::default(), |c| Rect {
        x: 0.,
        y: 0.,
        width: c.surface.width as f64,
        height: c.surface.height as f64,
    });
    let clip = s.controls.display.rect(clip).intersect(target);
    Ok(ViewGeometry {
        view_id: view.identity,
        view_generation: view.generation,
        scene_generation: s.controls.generation,
        x: transform.x,
        y: transform.y,
        scale_x: transform.sx,
        scale_y: transform.sy,
        clip: wire(clip),
        visible: visible
            && clip.width > 0.
            && clip.height > 0.
            && s.snapshots.get(&owner).is_some_and(|s| !s.items.is_empty()),
    })
}
pub(crate) fn empty_geometry() -> ViewGeometry {
    ViewGeometry {
        view_id: 0,
        view_generation: 0,
        scene_generation: 0,
        x: 0.,
        y: 0.,
        scale_x: 1.,
        scale_y: 1.,
        clip: wire(Rect::default()),
        visible: false,
    }
}
pub fn poll(s: &mut Scene) {
    for shell in [false, true] {
        let mut clients = if shell {
            std::mem::take(&mut s.controls.shell)
        } else {
            std::mem::take(&mut s.controls.accessibility)
        };
        clients.retain(|client| match client.channel.try_recv() {
            Ok(message) => {
                if let Some((ordinal, bytes)) = rt::envelope(&message.bytes) {
                    if client.allows(ordinal) {
                        handle(s, client.channel, shell, ordinal, bytes, &message.handles);
                    }
                }
                rt::close(&message.handles);
                true
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                if shell {
                    s.input.remove_shell(client.channel.0);
                }
                let _ = Memory::close(client.channel.0);
                false
            }
            Err(_) => true,
        });
        if shell {
            s.controls.shell = clients;
        } else {
            s.controls.accessibility = clients;
        }
    }
}
fn handle(
    s: &mut Scene,
    channel: Channel,
    shell: bool,
    ordinal: u64,
    bytes: &[u8],
    handles: &[u64],
) {
    let hs = rt::refs(handles);
    let result = (|| {
        // These global controls predate the authenticated shell subtree model.
        // Production shells use their scoped Flatland operations instead.
        if s.shell.enabled {
            return Err(Status::ErrAccessDenied);
        }
        let invalid = Status::ErrInvalidArgs;
        if shell {
            if ordinal == 5 {
                if !handles.is_empty() {
                    return Err(invalid);
                }
                let q =
                    ShellControlBeginMeasurementRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                if !(1..=5).contains(&q.workload) {
                    return Err(invalid);
                }
                if s.pending_frame.is_some() || s.gpu.pending.is_some() || s.scanout.client.busy() {
                    return Err(Status::ErrBusy);
                }
                *s.metrics = crate::metrics::Metrics::measurement(q.workload);
                return Ok(false);
            }
            if ordinal == 3 {
                if !handles.is_empty() {
                    return Err(invalid);
                }
                let q = ShellControlConfigureReservedEdgesRequest::decode(bytes, &hs)
                    .map_err(|_| invalid)?;
                if s.input.gesture_owner != 0 && s.input.gesture_owner != channel.0 {
                    return Err(Status::ErrAccessDenied);
                }
                s.input
                    .configure_edges(
                        channel.0,
                        bexos_flatland_input::gestures::EdgePolicy {
                            edges: q.edges,
                            inset: q.inset,
                            threshold: q.threshold,
                        },
                    )
                    .map_err(|_| invalid)?;
                return Ok(false);
            }
            if ordinal == 4 {
                if !handles.is_empty() {
                    return Err(invalid);
                }
                ShellControlReadGesturesRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                if s.input.gesture_owner != channel.0 {
                    return Err(Status::ErrAccessDenied);
                }
                let mut events = [crate::input::wire(bexos_flatland_input::Event::Reset); 8];
                let mut count = 0;
                while count < events.len() {
                    let Some(event) = s.input.shell_events.pop() else {
                        break;
                    };
                    events[count] = crate::input::wire(event);
                    count += 1;
                }
                rt::reply(
                    channel,
                    &ShellControlReadGesturesResponse {
                        status: Status::Ok,
                        events: WireVector::from_slice(&events[..count]),
                    },
                );
                return Ok(true);
            }
            if handles.len() != 1 {
                return Err(invalid);
            }
            let token = match ordinal {
                1 => {
                    ShellControlFocusViewRequest::decode(bytes, &hs)
                        .map_err(|_| invalid)?
                        .token
                }
                2 => {
                    ShellControlRaiseViewRequest::decode(bytes, &hs)
                        .map_err(|_| invalid)?
                        .token
                }
                _ => return Err(invalid),
            };
            let owner = s.controls.lookup(token.raw)?;
            if !geometry(s, owner)?.visible {
                return Err(invalid);
            }
            if ordinal == 1 {
                s.input.focus(owner);
            } else {
                if s.controls.embedded.contains(&owner) {
                    return Err(invalid);
                }
                let stacking = s
                    .controls
                    .pending_stacking
                    .get_or_insert_with(|| s.controls.stacking.clone());
                stacking.retain(|id| *id != owner);
                stacking.push(owner);
            }
        } else {
            match ordinal {
                1 => {
                    if handles.len() != 1 {
                        return Err(invalid);
                    }
                    let q = AccessibilityControlGetViewGeometryRequest::decode(bytes, &hs)
                        .map_err(|_| invalid)?;
                    let g = geometry(s, s.controls.lookup(q.token.raw)?)?;
                    rt::reply(
                        channel,
                        &AccessibilityControlGetViewGeometryResponse {
                            status: Status::Ok,
                            geometry: g,
                        },
                    );
                    return Ok(true);
                }
                2 => {
                    if handles.len() != 1 {
                        return Err(invalid);
                    }
                    let q = AccessibilityControlSetFocusRingRequest::decode(bytes, &hs)
                        .map_err(|_| invalid)?;
                    let owner = s.controls.lookup(q.token.raw)?;
                    if s.controls.views[&owner].generation != q.view_generation
                        || !geometry(s, owner)?.visible
                        || !valid_rect(rect(q.bounds))
                        || !(1..=16).contains(&q.width)
                        || q.rgba & 255 != 255
                    {
                        return Err(invalid);
                    }
                    s.controls.pending_ring = Some(Some(FocusRing {
                        view: owner,
                        generation: q.view_generation,
                        bounds: rect(q.bounds),
                        width: q.width,
                        rgba: q.rgba,
                    }));
                }
                3 => {
                    if !handles.is_empty() {
                        return Err(invalid);
                    }
                    AccessibilityControlClearFocusRingRequest::decode(bytes, &hs)
                        .map_err(|_| invalid)?;
                    s.controls.pending_ring = Some(None);
                }
                4 => {
                    if !handles.is_empty() {
                        return Err(invalid);
                    }
                    let q = AccessibilityControlSetDisplayTransformRequest::decode(bytes, &hs)
                        .map_err(|_| invalid)?;
                    let display = DisplayTransform {
                        scale: q.scale,
                        origin_x: q.origin_x,
                        origin_y: q.origin_y,
                        filter: q.color_filter,
                    };
                    display.validate().map_err(|_| invalid)?;
                    let canvas = s.canvas.as_ref().ok_or(invalid)?;
                    if display.origin_x
                        > canvas.surface.width as f64 - canvas.surface.width as f64 / display.scale
                        || display.origin_y
                            > canvas.surface.height as f64
                                - canvas.surface.height as f64 / display.scale
                    {
                        return Err(invalid);
                    }
                    if display != DisplayTransform::default() && s.controls.scratch.is_none() {
                        s.controls.scratch = Some(
                            rt::Mapping::new(canvas.output.size)
                                .map_err(|_| Status::ErrNoMemory)?,
                        );
                    }
                    s.controls.pending_display = Some(display);
                }
                _ => return Err(invalid),
            }
        }
        Ok(false)
    })();
    if result == Ok(true) {
        return;
    }
    let status = result.map_or_else(|s| s, |_| Status::Ok);
    if shell && ordinal == 4 {
        rt::reply(
            channel,
            &ShellControlReadGesturesResponse {
                status,
                events: WireVector::from_slice(&[]),
            },
        );
    } else if !shell && ordinal == 1 {
        rt::reply(
            channel,
            &AccessibilityControlGetViewGeometryResponse {
                status,
                geometry: empty_geometry(),
            },
        );
    } else {
        rt::reply(channel, &FlatlandSessionPresentResponse { status });
    }
}
