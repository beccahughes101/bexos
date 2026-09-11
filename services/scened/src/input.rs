//! Input aggregation is owned by scened; device and view identities come from grants.
use bexos_flatland_input::{
    Delivery, Event, Key, Phase,
    gestures::{Decision, EdgePolicy, Gestures},
    queue::EventQueue,
    router::Router,
    virtio::{Device, RawEvent},
};
use bexos_graphics_runtime as rt;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, Memory, live_migration::Resource};
use input_fidl::{FidlDecode, FidlEncode};
use std::collections::BTreeMap;
pub struct Link {
    pub control: Channel,
    pub reports: Channel,
    pub expected: Option<u64>,
    pub device: Device,
    pub subscription_deadline: Option<u64>,
}
#[derive(Default)]
pub struct Input {
    // Rebuilt from authenticated shell bindings before polling input.
    pub shell_views: std::collections::BTreeSet<u64>,
    pub pending_controls: Vec<Channel>,
    pub devices: BTreeMap<u64, Link>,
    pub router: Router,
    pub queues: BTreeMap<u64, EventQueue<128>>,
    pub delivered: u64,
    pub gestures: Gestures,
    pub shell_events: EventQueue<128>,
    pub gesture_owner: u64,
    pub settings: bexos_flatland_input::settings::Settings,
}
impl Input {
    pub fn configure_edges(&mut self, owner: u64, policy: EdgePolicy) -> Result<(), Error> {
        if owner == 0 || (self.gesture_owner != 0 && self.gesture_owner != owner) {
            return Err(Error::InvalidData);
        }
        let mut canceled = [None; 64];
        let mut count = 0;
        self.gestures.configure(policy, |p| {
            canceled[count] = Some(p);
            count += 1;
        })?;
        for p in canceled.into_iter().take(count).flatten() {
            let before = self.shell_events.overflows;
            self.shell_pointer(p);
            if before != self.shell_events.overflows {
                break;
            }
        }
        self.gesture_owner = owner;
        Ok(())
    }
    pub fn remove_shell(&mut self, owner: u64) {
        if self.gesture_owner == owner {
            self.gestures = Gestures::default();
            self.shell_events = EventQueue::default();
            self.gesture_owner = 0;
        }
    }
    fn shell_pointer(&mut self, p: bexos_flatland_input::Pointer) {
        let before = self.shell_events.overflows;
        self.shell_events.push(Event::Pointer(p));
        if before != self.shell_events.overflows {
            // The Reset ends every shell stream. Discard recognizer contacts so
            // no subsequent release arrives without a matching new press.
            let policy = self.gestures.policy;
            let _ = self.gestures.configure(policy, |_| {});
        }
    }
    pub fn arbitrate(&mut self, p: bexos_flatland_input::Pointer, width: f64, height: f64) -> bool {
        match self.gestures.pointer(p, width, height) {
            Decision::Pass => false,
            Decision::Claim { start, event } => {
                if let Some(cancel) = self.router.take_gesture(p.device, p.id) {
                    self.deliver(cancel);
                }
                let before = self.shell_events.overflows;
                self.shell_pointer(start);
                if before == self.shell_events.overflows {
                    self.shell_pointer(event);
                }
                true
            }
            Decision::Owned(p) => {
                self.shell_pointer(p);
                true
            }
        }
    }
    fn reset_gestures(&mut self, device: u64) {
        let mut canceled = [None; 64];
        let mut count = 0;
        self.gestures.remove_device(device, |p| {
            canceled[count] = Some(p);
            count += 1;
        });
        for p in canceled.into_iter().take(count).flatten() {
            let before = self.shell_events.overflows;
            self.shell_pointer(p);
            if before != self.shell_events.overflows {
                break;
            }
        }
    }
    pub fn focus(&mut self, view: u64) {
        let mut pending = [None; 256];
        let mut count = 0;
        self.router.focus(Some(view), |d| {
            pending[count] = Some(d);
            count += 1;
        });
        for delivery in pending.into_iter().take(count).flatten() {
            self.deliver(delivery);
        }
    }
    fn cancel_view_state(&mut self, view: u64) {
        self.router
            .captures
            .retain(|_, capture| capture.view != view);
        if self.router.focused == Some(view) {
            self.router.blocked.extend(self.router.keys.keys().copied());
            self.router.keys.clear();
        }
    }
    pub fn reset_view(&mut self, view: u64) {
        if self.queues.len() >= 16 && !self.queues.contains_key(&view) {
            return;
        }
        let queue = self.queues.entry(view).or_default();
        let overflows = queue.overflows.saturating_add(1);
        *queue = EventQueue::default();
        queue.overflows = overflows;
        queue.push(Event::Reset);
        self.cancel_view_state(view);
    }
    pub fn reconcile(&mut self, snapshots: &BTreeMap<u64, bexos_graphics::resolved::Snapshot>) {
        let mut pending = [None; 320];
        let mut count = 0;
        self.router.reconcile(snapshots, |delivery| {
            pending[count] = Some(delivery);
            count += 1;
        });
        for delivery in pending.into_iter().take(count).flatten() {
            self.deliver(delivery);
        }
    }
    pub fn queue_control(&mut self, control: Channel) -> Result<(), Error> {
        if control.0 == 0
            || self.pending_controls.len() + self.devices.len() >= 16
            || self.pending_controls.iter().any(|c| c.0 == control.0)
            || self.devices.contains_key(&control.0)
        {
            return Err(Error::Capacity);
        }
        self.pending_controls.push(control);
        Ok(())
    }

    pub fn bind_one_pending(&mut self) -> Result<bool, Error> {
        let Some(control) = self.pending_controls.pop() else {
            return Ok(false);
        };
        match self.connect(control) {
            Ok(()) => Ok(true),
            Err(error) => {
                let _ = Memory::close(control.0);
                Err(error)
            }
        }
    }

    pub fn connect(&mut self, control: Channel) -> Result<(), Error> {
        if self.devices.len() >= 16 || self.devices.contains_key(&control.0) {
            return Err(Error::Capacity);
        }
        let (local, remote) = Channel::pair().map_err(|_| Error::InvalidData)?;
        let request = input_fidl::InputDeviceSubscribeRequest {
            sink: input_fidl::HandleRef { raw: remote.0 },
        };
        let mut bytes = [0; 128];
        bytes[..8].copy_from_slice(&1u64.to_le_bytes());
        let mut hs = [input_fidl::HandleRef { raw: 0 }; 1];
        let encoded = match request.encode(&mut bytes[8..], &mut hs) {
            Ok(encoded) => encoded,
            Err(_) => {
                rt::close(&[local.0, remote.0]);
                return Err(Error::InvalidData);
            }
        };
        if control
            .send(&bytes[..8 + encoded.bytes], &[remote.0])
            .is_err()
        {
            rt::close(&[local.0, remote.0]);
            return Err(Error::InvalidData);
        }
        let mut device = Device::new(control.0);
        if let Err(error) = self.settings.apply(&mut device) {
            let _ = Memory::close(local.0);
            return Err(error);
        }
        self.devices.insert(
            control.0,
            Link {
                control,
                reports: local,
                device,
                expected: None,
                subscription_deadline: Some(rt::now_us().saturating_add(2_000_000)),
            },
        );
        Ok(())
    }

    pub fn route(
        &mut self,
        event: Event,
        snapshots: &BTreeMap<u64, bexos_graphics::resolved::Snapshot>,
    ) {
        self.route_ordered(event, snapshots, &[], None);
    }
    fn route_ordered(
        &mut self,
        event: Event,
        snapshots: &BTreeMap<u64, bexos_graphics::resolved::Snapshot>,
        stacking: &[u64],
        world: Option<&bexos_graphics::world::World>,
    ) {
        match event {
            Event::Pointer(p) => {
                let delivery = if let Some(world) = world {
                    self.router.pointer_items(p, world.items(snapshots))
                } else {
                    self.router.pointer(
                        p,
                        snapshots
                            .iter()
                            .filter(|(id, _)| !stacking.contains(id))
                            .map(|(id, s)| (*id, s))
                            .chain(
                                stacking
                                    .iter()
                                    .filter_map(|id| snapshots.get(id).map(|s| (*id, s))),
                            ),
                    )
                };
                if let Some(mut d) = delivery {
                    if !self.accept_visible_delivery(d.view, world) {
                        return;
                    }
                    if self.shell_views.contains(&d.view) {
                        d.event = Event::Pointer(p);
                    }
                    if matches!(d.event,Event::Pointer(p) if p.phase==Phase::Down) {
                        self.focus(d.view);
                    }
                    self.deliver(d);
                }
            }
            Event::Key(k) => {
                if let Some(d) = self.router.key(k) {
                    if self.accept_visible_delivery(d.view, world) {
                        self.deliver(d);
                    }
                }
            }
            Event::Reset => {
                let mut views = [0; 16];
                let count = self.queues.len();
                for (view, id) in views.iter_mut().zip(self.queues.keys()) {
                    *view = *id;
                }
                for view in views.into_iter().take(count) {
                    self.reset_view(view);
                }
                self.router.captures.clear();
            }
        }
    }
    fn accept_visible_delivery(
        &mut self,
        view: u64,
        world: Option<&bexos_graphics::world::World>,
    ) -> bool {
        if world.is_none_or(|world| world.paint.iter().any(|p| p.view == view)) {
            return true;
        }
        // Focus can be requested before an embedding is presented. Never queue
        // input for later replay into that currently hidden subtree.
        self.reset_view(view);
        if self.router.focused == Some(view) {
            self.router.focused = None;
        }
        false
    }
    fn deliver(&mut self, d: Delivery) {
        if self.queues.len() >= 16 && !self.queues.contains_key(&d.view) {
            return;
        }
        let queue = self.queues.entry(d.view).or_default();
        let before = queue.overflows;
        queue.push(d.event);
        if queue.overflows != before {
            self.cancel_view_state(d.view);
        }
        self.delivered = self.delivered.saturating_add(1);
        if self.delivered <= 16 {
            bexos_userspace::log(&format!(
                "scened: input routed view={} kind={}\n",
                d.view,
                match d.event {
                    Event::Key(_) => "key",
                    Event::Pointer(_) => "pointer",
                    Event::Reset => "reset",
                }
            ));
        }
    }
    pub fn poll(
        &mut self,
        snapshots: &BTreeMap<u64, bexos_graphics::resolved::Snapshot>,
        width: f64,
        height: f64,
        now: u64,
        stacking: &[u64],
        display: bexos_graphics::accessibility::DisplayTransform,
        world: &bexos_graphics::world::World,
    ) {
        // Move normalized reports into bounded stack storage before routing so
        // mutable device state never aliases the committed scene or client queues.
        let mut ids = [0; 16];
        let count = self.devices.len();
        for (slot, id) in ids.iter_mut().zip(self.devices.keys()) {
            *slot = *id;
        }
        for id in ids.into_iter().take(count) {
            let mut normalized = EventQueue::<256>::default();
            let mut closed = false;
            let link = self.devices.get_mut(&id).unwrap();
            match link.poll_subscription(now) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => {
                    self.remove_device(id);
                    continue;
                }
            }
            let mut storage = [0; 4096];
            for _ in 0..4 {
                match rt::stream::read_no_handles(link.reports, &mut storage) {
                    Ok(bytes) => match input_fidl::InputReport::decode(bytes, &[]) {
                        Ok(report) => {
                            if report.reset || link.expected.is_some_and(|n| n != report.sequence) {
                                let _ = link.device.feed_at(
                                    RawEvent {
                                        kind: 0,
                                        code: 3,
                                        value: 0,
                                    },
                                    width,
                                    height,
                                    now,
                                );
                                normalized.push(Event::Reset);
                                if link.expected.is_none() {
                                    link.device.dropping = false;
                                }
                            }
                            link.expected = Some(report.sequence.wrapping_add(1));
                            for i in 0..report.events.len() {
                                match report.events.get(i) {
                                    Ok(raw) => link.device.feed_report(
                                        RawEvent {
                                            kind: raw.kind,
                                            code: raw.code,
                                            value: raw.value,
                                        },
                                        width,
                                        height,
                                        now,
                                        |e| normalized.push(e),
                                    ),
                                    Err(_) => {
                                        let _ = link.device.feed_at(
                                            RawEvent {
                                                kind: 0,
                                                code: 3,
                                                value: 0,
                                            },
                                            width,
                                            height,
                                            now,
                                        );
                                        normalized.push(Event::Reset);
                                        break;
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            let _ = link.device.feed_at(
                                RawEvent {
                                    kind: 0,
                                    code: 3,
                                    value: 0,
                                },
                                width,
                                height,
                                now,
                            );
                            normalized.push(Event::Reset);
                        }
                    },
                    Err(
                        kernel_fidl::Status::ErrPeerClosed
                        | kernel_fidl::Status::ErrBufferTooSmall
                        | kernel_fidl::Status::ErrInvalidArgs,
                    ) => {
                        closed = true;
                        break;
                    }
                    Err(_) => break,
                }
            }
            if let Some(e) = link.device.repeat(now, width, height) {
                normalized.push(e);
            }
            while let Some(event) = normalized.pop() {
                if event == Event::Reset {
                    self.reset_gestures(id);
                    self.reset_device_routes(id);
                } else {
                    let event = if let Event::Pointer(mut p) = event {
                        if self.arbitrate(p, width, height) {
                            continue;
                        }
                        (p.x, p.y) = display.local(p.x, p.y);
                        Event::Pointer(p)
                    } else {
                        event
                    };
                    self.route_ordered(event, snapshots, stacking, Some(world));
                }
            }
            if closed {
                self.remove_device(id);
            }
        }
    }
    pub fn remove_device(&mut self, id: u64) {
        self.reset_gestures(id);
        self.reset_device_routes(id);
        if let Some(link) = self.devices.remove(&id) {
            rt::close(&[link.control.0, link.reports.0]);
        }
    }
    fn reset_device_routes(&mut self, id: u64) {
        let mut canceled = [None; 320];
        let mut count = 0;
        self.router.reset_device(id, |d| {
            canceled[count] = Some(d);
            count += 1;
        });
        for d in canceled.into_iter().take(count).flatten() {
            self.deliver(d);
        }
    }
    pub fn remove_view(&mut self, id: u64) {
        self.queues.remove(&id);
        self.router.remove_view(id);
    }
    pub fn resources(&self) -> Vec<Resource> {
        self.devices
            .values()
            .flat_map(|l| [Resource::Handle(l.control.0), Resource::Handle(l.reports.0)])
            .chain(self.pending_controls.iter().map(|c| Resource::Handle(c.0)))
            .collect()
    }
    pub fn encode(&self) -> Vec<u8> {
        self.encode_secure(false)
    }
    pub fn encode_secure(&self, secure: bool) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(5);
        w.word(self.delivered);
        if secure {
            Router {
                focused: self.router.focused,
                captures: self.router.captures.clone(),
                ..Default::default()
            }
            .encode(&mut w);
        } else {
            self.router.encode(&mut w);
        }
        if self.pending_controls.len() > 16 {
            return w.finish();
        }
        w.word(self.pending_controls.len() as u64);
        for control in &self.pending_controls {
            w.word(control.0);
        }
        w.word(self.devices.len() as u64);
        for (id, l) in &self.devices {
            w.word(*id);
            w.word(l.control.0);
            w.word(l.reports.0);
            w.word(l.expected.is_some() as u64);
            if let Some(n) = l.expected {
                w.word(n)
            }
            l.device.encode_keyboard(&mut w, !secure);
            w.word(l.subscription_deadline.unwrap_or(0));
        }
        w.word(self.queues.len() as u64);
        for (id, q) in &self.queues {
            w.word(*id);
            if secure {
                EventQueue::<128>::default().encode(&mut w);
            } else {
                q.encode(&mut w);
            }
        }
        w.word(self.gesture_owner);
        self.gestures.encode(&mut w);
        self.shell_events.encode(&mut w);
        self.settings.encode(&mut w);
        w.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        let version = r.word()?;
        if !(1..=5).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        let mut out = Self::default();
        out.delivered = r.word()?;
        out.router = Router::decode(&mut r)?;
        if version >= 5 {
            for _ in 0..r.count(16)? {
                let raw = r.word()?;
                if raw == 0 || out.pending_controls.iter().any(|c| c.0 == raw) {
                    return Err(Error::InvalidData);
                }
                out.pending_controls.push(Channel(raw));
            }
        }
        for _ in 0..r.count(16)? {
            let id = r.word()?;
            let control = Channel(r.word()?);
            let reports = Channel(r.word()?);
            let expected = if r.flag()? { Some(r.word()?) } else { None };
            let device = Device::decode(&mut r)?;
            let deadline = if version >= 4 { r.word()? } else { 0 };
            let subscription_deadline = (deadline != 0).then_some(deadline);
            if id == 0
                || id != control.0
                || reports.0 == 0
                || device.id != id
                || out
                    .devices
                    .insert(
                        id,
                        Link {
                            control,
                            reports,
                            expected,
                            device,
                            subscription_deadline,
                        },
                    )
                    .is_some()
            {
                return Err(Error::InvalidData);
            }
        }
        for _ in 0..r.count(16)? {
            let id = r.word()?;
            let q = EventQueue::decode(&mut r)?;
            if id == 0 || out.queues.insert(id, q).is_some() {
                return Err(Error::InvalidData);
            }
        }
        if version >= 2 {
            out.gesture_owner = r.word()?;
            out.gestures = Gestures::decode(&mut r)?;
            out.shell_events = EventQueue::decode(&mut r)?;
            if out.gesture_owner == 0
                && (out.gestures.policy.edges != 0 || !out.shell_events.is_empty())
            {
                return Err(Error::InvalidData);
            }
        }
        if version >= 3 {
            out.settings = bexos_flatland_input::settings::Settings::decode(&mut r)?;
        }
        r.finish()?;
        Ok(out)
    }
}
pub fn wire(e: Event) -> graphics_fidl::NormalizedInputEvent {
    let mut out = graphics_fidl::NormalizedInputEvent {
        kind: 0,
        device: 0,
        id: 0,
        phase: 0,
        x: 0.,
        y: 0.,
        buttons: 0,
        scroll_x: 0.,
        scroll_y: 0.,
        code: 0,
        key_state: 0,
        modifiers: 0,
        unicode: 0,
    };
    match e {
        Event::Reset => {}
        Event::Pointer(p) => {
            out.kind = 1;
            out.device = p.device;
            out.id = p.id;
            out.phase = p.phase as u32;
            out.x = p.x;
            out.y = p.y;
            out.buttons = p.buttons;
            out.scroll_x = p.scroll_x;
            out.scroll_y = p.scroll_y;
        }
        Event::Key(Key {
            device,
            code,
            state,
            modifiers,
            unicode,
        }) => {
            out.kind = 2;
            out.device = device;
            out.code = code;
            out.key_state = state as u32;
            out.modifiers = modifiers;
            out.unicode = unicode;
        }
    }
    out
}

#[cfg(test)]
mod secure_tests {
    use super::*;
    #[test]
    fn unpresented_focus_cannot_queue_keys_for_later_attachment() {
        let mut input = Input::default();
        input.router.focused = Some(7);
        input.route_ordered(
            Event::Key(Key {
                device: 1,
                code: 30,
                state: 1,
                unicode: 97,
                modifiers: 0,
            }),
            &BTreeMap::new(),
            &[],
            Some(&bexos_graphics::world::World::default()),
        );
        assert_eq!(input.router.focused, None);
        assert!(input.router.keys.is_empty());
        assert!(input.router.blocked.contains(&(1, 30)));
        let queue = input.queues.get_mut(&7).unwrap();
        assert_eq!(queue.pop(), Some(Event::Reset));
        assert_eq!(queue.pop(), None);
    }
    #[test]
    fn reset_removes_queued_credentials_and_active_captures() {
        use bexos_flatland_input::{Event, Key, Phase, Pointer, router::Capture};
        let mut input = Input::default();
        let p = Pointer {
            device: 1,
            id: 2,
            phase: Phase::Down,
            x: 10.,
            y: 20.,
            buttons: 1,
            ..Default::default()
        };
        input.router.captures.insert(
            (1, 2),
            Capture {
                view: 7,
                node: 1,
                last: p,
            },
        );
        let key = Key {
            device: 1,
            code: 30,
            state: 1,
            unicode: 97,
            modifiers: 0,
        };
        input.router.focused = Some(7);
        input.router.keys.insert((1, 30), key);
        input.queues.entry(7).or_default().push(Event::Key(key));
        let restored = Input::decode(&input.encode_secure(true)).unwrap();
        assert!(restored.router.keys.is_empty());
        assert!(restored.queues.get(&7).is_some_and(|q| q.is_empty()));
        input.reset_view(7);
        assert!(input.router.captures.is_empty());
        assert!(input.router.keys.is_empty());
        assert!(input.router.blocked.contains(&(1, 30)));
        assert_eq!(input.queues.get_mut(&7).unwrap().pop(), Some(Event::Reset));
        assert!(input.queues.get_mut(&7).unwrap().pop().is_none());
    }
}
