mod render;
#[cfg(test)]
mod tests;
use bexos_dioxus_guest::{View, bexos::wasm::kernel, rpc};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_shell_guest::{Client, Listener, Snapshot, paint, windows::Window};
// Keep the resize grip outside the embedded app so the desktop owns its input.
const TITLE_HEIGHT: u32 = 32;
const RESIZE_BORDER: u32 = 14;
const VERTICAL_CHROME: u32 = TITLE_HEIGHT + RESIZE_BORDER;

#[derive(Default)]
pub struct Desktop {
    view: Option<View>,
    client: Option<Client>,
    snapshot: Snapshot,
    listener: Option<Listener>,
    windows: Vec<Window>,
    next_node: u64,
    width: u32,
    height: u32,
    next_poll: u64,
    launcher: bool,
    launcher_offset: usize,
    drag: Option<(u64, f64, f64, bool)>,
    pub error: String,
    pub dirty: bool,
}
impl Desktop {
    pub fn tick(&mut self) -> Result<(), String> {
        if self.view.is_none() {
            self.view = Some(View::open(800, 600)?);
            self.width = 800;
            self.height = 600;
            self.next_node = 100;
            self.dirty = true;
        }
        if self.client.is_none() {
            self.client = Some(Client::connect()?);
        }
        if self.listener.is_none() {
            self.listener = Some(self.client.as_ref().unwrap().listen()?);
        }
        if let Some(phase) = self.listener.as_mut().unwrap().poll()? {
            self.snapshot.state = phase;
            if phase != 1 {
                self.drag = None;
                self.launcher = false;
            }
            self.dirty = true;
        }
        let view = self.view.as_ref().unwrap();
        let client = self.client.as_ref().unwrap();
        let now = kernel::monotonic_ns();
        if now >= self.next_poll {
            let snapshot = client.snapshot()?;
            if snapshot != self.snapshot {
                self.snapshot = snapshot;
                self.dirty = true;
            }
            let (width, height, children) = view.shell_views()?;
            if (width, height) != (self.width, self.height) {
                self.width = width;
                self.height = height;
                view.configure(width, height, 1.0)?;
                self.dirty = true;
            }
            let ids = children.iter().map(|c| c.id).collect::<Vec<_>>();
            for w in self.windows.iter().filter(|w| !ids.contains(&w.view.id)) {
                view.destroy_node(w.node, w.node + 1)?;
                view.remove_child(w.node)?;
                rpc::close(w.view.token);
                self.dirty = true;
            }
            self.windows.retain(|w| ids.contains(&w.view.id));
            let mut added_window = false;
            for child in children {
                if let Some(w) = self.windows.iter_mut().find(|w| w.view.id == child.id) {
                    if (w.view.width, w.view.height) != (child.width, child.height) {
                        self.dirty = true;
                    }
                    rpc::close(w.view.token);
                    w.view = child;
                    continue;
                }
                let node = self.next_node;
                self.next_node += 2;
                let offset = (self.windows.len() as i32 % 6) * 28;
                let mut w = Window {
                    width: child.width.min(560).max(240),
                    height: (child.height + VERTICAL_CHROME).min(400).max(160),
                    view: child,
                    node,
                    x: 40 + offset,
                    y: 32 + offset,
                };
                w.constrain(width, height);
                view.create_node(node, view.root_node())?;
                view.embed_at(
                    node,
                    node + 1,
                    &w.view,
                    4,
                    TITLE_HEIGHT as i32,
                    w.width - 8,
                    w.height - VERTICAL_CHROME,
                    true,
                )?;
                view.focus_child(&w.view)?;
                self.windows.push(w);
                added_window = true;
                self.dirty = true;
            }
            // The enumeration predates our focus request for a new window.
            // Do not put the previously focused window back on top using it.
            if self.drag.is_none() && !added_window {
                if let Some(i) = self.windows.iter().position(|w| w.view.focused) {
                    if i + 1 != self.windows.len() {
                        let w = self.windows.remove(i);
                        self.windows.push(w);
                        self.dirty = true;
                    }
                }
            }
            self.next_poll = now + 300_000_000;
            self.launcher_offset = self
                .launcher_offset
                .min(self.snapshot.apps.len().saturating_sub(10));
        }
        for e in view.input()? {
            if self.snapshot.state != 1 {
                continue;
            }
            if e.kind == 1 {
                if e.phase == 1 {
                    if e.y >= self.height as f64 - 44.0 {
                        if e.x < 108.0 {
                            self.launcher = !self.launcher;
                        } else if e.x > self.width as f64 - 100.0 {
                            client.action(3)?;
                        } else if e.x > self.width as f64 - 212.0 {
                            client.action(5)?;
                        } else {
                            let i = ((e.x - 112.0) / 120.0) as usize;
                            if i < self.windows.len() {
                                let w = self.windows.remove(i);
                                view.focus_child(&w.view)?;
                                self.windows.push(w);
                            }
                        }
                    } else if self.launcher && e.x < 340.0 {
                        let index = self.launcher_offset + ((e.y - 20.0) / 38.0) as usize;
                        if let Some((package, _)) = self.snapshot.apps.get(index) {
                            if let Err(err) = client.open_app(package) {
                                self.error = err;
                            }
                            self.launcher = false;
                            self.next_poll = 0;
                        }
                    } else if let Some(i) = self.windows.iter().rposition(|w| {
                        paint::inside(
                            e.x,
                            e.y,
                            (w.x as f32, w.y as f32, w.width as f32, w.height as f32),
                        )
                    }) {
                        let w = self.windows.remove(i);
                        let resize = e.x > (w.x as f64 + w.width as f64 - RESIZE_BORDER as f64)
                            && e.y > (w.y as f64 + w.height as f64 - RESIZE_BORDER as f64);
                        let closing =
                            e.y < (w.y + 32) as f64 && e.x > (w.x as f64 + w.width as f64 - 32.0);
                        if closing {
                            client.close_app(&w.view.package)?;
                            self.next_poll = 0;
                        } else if resize || e.y < (w.y + 32) as f64 {
                            self.drag = Some((w.view.id, e.x, e.y, resize));
                        }
                        if !closing {
                            view.focus_child(&w.view)?;
                        }
                        self.windows.push(w);
                    }
                } else if let Some((id, x, y, resize)) = self.drag {
                    if e.phase == 3 || e.phase == 5 {
                        self.drag = None;
                    } else if let Some(w) = self.windows.iter_mut().find(|w| w.view.id == id) {
                        if resize {
                            w.width = (w.width as f64 + e.x - x).max(160.0) as u32;
                            w.height = (w.height as f64 + e.y - y).max(100.0) as u32;
                        } else {
                            w.x += (e.x - x) as i32;
                            w.y += (e.y - y) as i32;
                        }
                        w.constrain(self.width, self.height);
                        self.drag = Some((id, e.x, e.y, resize));
                    }
                }
                self.dirty = true;
            }
            if e.kind == 2 && e.key_state == 1 && e.unicode == 27 {
                self.launcher = false;
                self.drag = None;
                self.dirty = true;
            }
            if self.launcher && e.scroll_y != 0.0 {
                self.launcher_offset = if e.scroll_y > 0.0 {
                    self.launcher_offset.saturating_sub(1)
                } else {
                    self.launcher_offset
                        .saturating_add(1)
                        .min(self.snapshot.apps.len().saturating_sub(10))
                };
                self.dirty = true;
            }
        }
        if self.dirty {
            self.render()?;
        }
        Ok(())
    }
    pub fn checkpoint(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(2);
        w.word(self.listener.as_ref().map_or(0, |v| v.channel as u64));
        w.word(self.listener.as_ref().map_or(0, |v| v.display_token as u64));
        w.word(self.view.as_ref().map_or(0, |v| v.id() as u64));
        w.word(self.client.as_ref().map_or(0, |v| v.0 as u64));
        w.word(self.next_node);
        w.word(self.width as u64);
        w.word(self.height as u64);
        w.word(self.windows.len() as u64);
        for v in &self.windows {
            v.encode(&mut w);
        }
        w.finish()
    }
    pub fn restore(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != 2 {
            return Err(Error::UnsupportedVersion);
        }
        let channel: u32 = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let display_token: u32 = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let listener = (channel != 0).then_some(Listener {
            channel,
            display_token,
        });
        let view: u32 = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let client: u32 = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let next_node = r.word()?;
        let width = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let height = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let mut windows = Vec::new();
        for _ in 0..r.count(14)? {
            let window = Window::decode(&mut r)?;
            if window.node < 100
                || window.node % 2 != 0
                || window.node.checked_add(1).is_none_or(|n| n >= next_node)
                || windows
                    .iter()
                    .any(|w: &Window| w.node == window.node || w.view.id == window.view.id)
            {
                return Err(Error::InvalidData);
            }
            windows.push(window);
        }
        r.finish()?;
        if width > 8192 || height > 8192 {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            listener,
            view: (view != 0).then(|| View::adopt(view)),
            client: (client != 0).then(|| Client(client)),
            next_node,
            width,
            height,
            windows,
            dirty: true,
            ..Default::default()
        })
    }
}
