mod render;
use bexos_dioxus_guest::{View, bexos::wasm::kernel, exports::bexos::wasm::lifecycle::Guest, rpc};
use bexos_migration::codec::{Decoder, Encoder};
use bexos_shell_guest::{Client, Snapshot, paint};
use std::cell::RefCell;
pub struct Service;
#[derive(Default)]
struct State {
    view: Option<View>,
    client: Option<Client>,
    snapshot: Snapshot,
    selected: usize,
    password: String,
    error: String,
    child: Option<u64>,
    attached: Option<u64>,
    width: u32,
    height: u32,
    next_poll: u64,
    dirty: bool,
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
impl Guest for Service {
    fn dispatch(_: u32) {
        STATE.with_borrow_mut(|s| {
            if let Err(e) = s.tick() {
                if s.error != e {
                    eprintln!("shell UI: {e}");
                }
                s.error = e;
                s.dirty = true;
            }
        });
    }
    fn checkpoint() -> Vec<u8> {
        STATE.with_borrow_mut(|s| {
            clear_password(&mut s.password);
            let mut w = Encoder::new();
            w.word(1);
            w.word(s.view.as_ref().map_or(0, |v| v.id() as u64));
            w.word(s.client.as_ref().map_or(0, |c| c.0 as u64));
            w.word(s.selected as u64);
            w.word(s.child.unwrap_or(0));
            w.word(s.width as u64);
            w.word(s.height as u64);
            w.finish()
        })
    }
    fn restore(bytes: Vec<u8>) -> Result<(), ()> {
        let mut r = Decoder::new(&bytes);
        if r.word().map_err(|_| ())? != 1 {
            return Err(());
        }
        let view = r.word().map_err(|_| ())?;
        let client = r.word().map_err(|_| ())?;
        let selected = r.count(64).map_err(|_| ())?;
        let child = r.word().map_err(|_| ())?;
        let width = r.word().map_err(|_| ())?;
        let height = r.word().map_err(|_| ())?;
        r.finish().map_err(|_| ())?;
        if view > u32::MAX as u64 || client > u32::MAX as u64 || width > 8192 || height > 8192 {
            return Err(());
        }
        STATE.with_borrow_mut(|s| {
            *s = State {
                view: (view != 0).then(|| View::adopt(view as u32)),
                client: (client != 0).then(|| Client(client as u32)),
                selected,
                child: (child != 0).then_some(child),
                width: width as u32,
                height: height as u32,
                dirty: true,
                ..Default::default()
            };
        });
        Ok(())
    }
    fn activate() {}
    fn abort() {}
}
impl State {
    fn tick(&mut self) -> Result<(), String> {
        // Reserve before accepting input so reallocations never leave old
        // password prefixes in freed allocations.
        if self.password.is_empty() && self.password.capacity() < 256 {
            self.password.reserve(256);
        }
        if self.view.is_none() {
            self.view = Some(View::open(800, 600)?);
            self.width = 800;
            self.height = 600;
            self.dirty = true;
        }
        if self.client.is_none() {
            self.client = Some(Client::connect()?);
        }
        let client = self.client.as_ref().unwrap();
        let view = self.view.as_ref().unwrap();
        let now = kernel::monotonic_ns();
        if now >= self.next_poll {
            let snapshot = client.snapshot()?;
            if snapshot != self.snapshot {
                self.snapshot = snapshot;
                self.dirty = true;
            }
            self.selected = self
                .selected
                .min(self.snapshot.users.len().saturating_sub(1));
            let (width, height, children) = view.shell_views()?;
            if (width, height) != (self.width, self.height) {
                view.configure(width, height, 1.0)?;
                self.width = width;
                self.height = height;
                self.dirty = true;
            }
            let next = children.first().map(|v| v.id);
            if next != self.child {
                if self.child.is_some() {
                    view.remove_child(100)?;
                }
                if let Some(child) = children.first() {
                    view.embed(100, child, 0, 0, width, height, true)?;
                }
                self.child = next;
                self.dirty = true;
            }
            if let Some(c) = children.first() {
                if self.attached != Some(c.id) && client.attach_display(c.token).is_ok() {
                    self.attached = Some(c.id);
                }
            } else {
                self.attached = None;
            }
            for c in children {
                rpc::close(c.token);
            }
            self.next_poll = now + 250_000_000;
        }
        let x = (self.width as f32 - 440.0) / 2.0;
        let y = (self.height as f32 - 280.0) / 2.0;
        for event in view.input()? {
            if self.snapshot.state == 1 {
                continue;
            }
            let mut login = false;
            if event.kind == 1 && event.phase == 1 {
                if self.snapshot.uid != 0
                    && paint::inside(event.x, event.y, (x + 200.0, y + 164.0, 180.0, 32.0))
                {
                    clear_password(&mut self.password);
                    self.error = client.action(5).err().unwrap_or_default();
                    self.next_poll = 0;
                    self.dirty = true;
                    continue;
                }
                if paint::inside(event.x, event.y, (x, y + 56.0, 440.0, 32.0))
                    && self.snapshot.uid == 0
                    && !self.snapshot.users.is_empty()
                {
                    self.selected = (self.selected + 1) % self.snapshot.users.len();
                    clear_password(&mut self.password);
                }
                login = paint::inside(event.x, event.y, (x, y + 164.0, 180.0, 32.0));
            }
            if event.kind == 2 && event.key_state == 1 {
                match event.unicode {
                    13 | 10 => login = true,
                    8 | 127 => {
                        if let Some((index, _)) = self.password.char_indices().next_back() {
                            unsafe {
                                rpc::clear_sensitive(&mut self.password.as_bytes_mut()[index..]);
                            }
                            self.password.truncate(index);
                        }
                    }
                    9 if self.snapshot.uid == 0 && !self.snapshot.users.is_empty() => {
                        self.selected = (self.selected + 1) % self.snapshot.users.len();
                        clear_password(&mut self.password);
                    }
                    c if c >= 32 => {
                        if let Some(c) = char::from_u32(c) {
                            if self.password.len() + c.len_utf8() <= 256 {
                                self.password.push(c);
                            }
                        }
                    }
                    _ => {}
                }
            }
            if login {
                if let Some((uid, _)) = self.snapshot.users.get(self.selected) {
                    let uid = if self.snapshot.uid != 0 {
                        self.snapshot.uid
                    } else {
                        *uid
                    };
                    let result = client.login(uid, &self.password);
                    // Overwrite the allocated bytes before releasing the credential buffer.
                    clear_password(&mut self.password);
                    self.error = result
                        .err()
                        .map_or(String::new(), |_| "SIGN IN FAILED - TRY AGAIN".into());
                    self.next_poll = 0;
                }
            }
            self.dirty = true;
        }
        if self.dirty {
            self.render()?;
        }
        Ok(())
    }
}

fn clear_password(password: &mut String) {
    unsafe {
        rpc::clear_sensitive(password.as_bytes_mut());
    }
    password.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_discards_credentials_and_retains_selection() {
        STATE.with_borrow_mut(|s| {
            s.password = "never-checkpoint-this-password".into();
            s.selected = 2;
        });
        let bytes = Service::checkpoint();
        assert!(!bytes.windows(5).any(|w| w == b"never"));
        STATE.with_borrow(|s| assert!(s.password.is_empty()));
        Service::restore(bytes).unwrap();
        STATE.with_borrow(|s| {
            assert!(s.password.is_empty());
            assert_eq!(s.selected, 2);
        });
    }
}
