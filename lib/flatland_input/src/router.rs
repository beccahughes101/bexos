use crate::{Delivery, Event, Phase, Pointer};
use alloc::{collections::BTreeMap, vec::Vec};
use bexos_flatland::resolved::Snapshot;
#[derive(Clone, Copy, Debug)]
pub struct Capture {
    pub view: u64,
    pub node: u64,
    pub last: Pointer,
}
#[derive(Default)]
pub struct Router {
    pub focused: Option<u64>,
    pub captures: BTreeMap<(u64, u32), Capture>,
    pub keys: BTreeMap<(u64, u32), crate::Key>,
    pub blocked: alloc::collections::BTreeSet<(u64, u32)>,
}
impl Router {
    /// Cancel captures as soon as a committed update removes their visible
    /// target, even when no further hardware report arrives for that contact.
    pub fn reconcile(
        &mut self,
        views: &BTreeMap<u64, Snapshot>,
        mut deliver: impl FnMut(Delivery),
    ) {
        self.captures.retain(|_, capture| {
            if views
                .get(&capture.view)
                .is_some_and(|snapshot| snapshot.items.iter().any(|item| item.node == capture.node))
            {
                return true;
            }
            deliver(Delivery {
                view: capture.view,
                event: Event::Pointer(Pointer {
                    phase: Phase::Cancel,
                    ..capture.last
                }),
            });
            false
        });
        if self.focused.is_some_and(|view| {
            views
                .get(&view)
                .is_none_or(|snapshot| snapshot.items.is_empty())
        }) {
            self.focus(None, deliver);
        }
    }
    pub fn pointer<'a>(
        &mut self,
        event: Pointer,
        views: impl DoubleEndedIterator<Item = (u64, &'a Snapshot)>,
    ) -> Option<Delivery> {
        self.pointer_items(
            event,
            views.flat_map(|(view, s)| s.items.iter().map(move |item| (view, item))),
        )
    }
    pub fn pointer_items<'a>(
        &mut self,
        event: Pointer,
        items: impl DoubleEndedIterator<Item = (u64, &'a bexos_flatland::resolved::Item)>,
    ) -> Option<Delivery> {
        if !event.x.is_finite() || !event.y.is_finite() {
            return None;
        }
        let key = (event.device, event.id);
        if !self.captures.contains_key(&key)
            && matches!(event.phase, Phase::Move | Phase::Up | Phase::Cancel)
        {
            return None;
        }
        let mut items = items;
        let hit = if let Some(c) = self.captures.get(&key) {
            items
                .find(|(id, i)| *id == c.view && i.node == c.node)
                .map(|(id, i)| {
                    let (x, y) = i.local(event.x, event.y);
                    (id, i.node, x, y)
                })
        } else {
            items.rev().find_map(|(id, i)| {
                if !i.contains(event.x, event.y) {
                    return None;
                }
                let (x, y) = i.local(event.x, event.y);
                Some((id, i.node, x, y))
            })
        };
        let Some((view, node, x, y)) = hit else {
            return self.captures.remove(&key).map(|c| Delivery {
                view: c.view,
                event: Event::Pointer(Pointer {
                    phase: Phase::Cancel,
                    ..c.last
                }),
            });
        };
        if event.phase == Phase::Down {
            if self.captures.contains_key(&key) || self.captures.len() >= 64 {
                return None;
            }
            self.captures.insert(
                key,
                Capture {
                    view,
                    node,
                    last: Pointer { x, y, ..event },
                },
            );
        } else if let Some(c) = self.captures.get_mut(&key) {
            c.last = Pointer { x, y, ..event };
        }
        if matches!(event.phase, Phase::Up | Phase::Cancel) {
            self.captures.remove(&key);
        }
        Some(Delivery {
            view,
            event: Event::Pointer(Pointer { x, y, ..event }),
        })
    }
    pub fn focus(&mut self, view: Option<u64>, mut deliver: impl FnMut(Delivery)) {
        if self.focused == view {
            return;
        }
        if let Some(old) = self.focused {
            for ((device, code), key) in &self.keys {
                deliver(Delivery {
                    view: old,
                    event: Event::Key(crate::Key {
                        state: 0,
                        unicode: 0,
                        ..*key
                    }),
                });
                self.blocked.insert((*device, *code));
            }
        }
        self.keys.clear();
        self.focused = view;
    }
    pub fn key(&mut self, event: crate::Key) -> Option<Delivery> {
        let id = (event.device, event.code);
        if self.blocked.contains(&id) {
            if event.state == 0 {
                self.blocked.remove(&id);
            }
            return None;
        }
        let view = self.focused?;
        if event.state == 0 {
            self.keys.remove(&id)?;
        } else if event.state == 1 {
            if self.keys.len() + self.blocked.len() >= 256 {
                return None;
            }
            self.keys.insert(id, event);
        } else if event.state != 2 || !self.keys.contains_key(&id) {
            return None;
        }
        Some(Delivery {
            view,
            event: Event::Key(event),
        })
    }
    pub fn reset_device(&mut self, device: u64, mut deliver: impl FnMut(Delivery)) {
        self.cancel_device(device, &mut deliver);
        let focused = self.focused;
        self.keys.retain(|(d, _), key| {
            if *d != device {
                return true;
            }
            if let Some(view) = focused {
                deliver(Delivery {
                    view,
                    event: Event::Key(crate::Key {
                        state: 0,
                        unicode: 0,
                        ..*key
                    }),
                });
            }
            false
        });
        self.blocked.retain(|(d, _)| *d != device);
    }
    pub fn cancel_device(&mut self, device: u64, mut deliver: impl FnMut(Delivery)) {
        self.captures.retain(|(d, _), c| {
            if *d != device {
                return true;
            }
            deliver(Delivery {
                view: c.view,
                event: Event::Pointer(Pointer {
                    phase: Phase::Cancel,
                    ..c.last
                }),
            });
            false
        });
    }
    /// Only a privileged shell gesture recognizer may invoke takeover.
    pub fn take_gesture(&mut self, device: u64, pointer: u32) -> Option<Delivery> {
        self.captures.remove(&(device, pointer)).map(|c| Delivery {
            view: c.view,
            event: Event::Pointer(Pointer {
                phase: Phase::Cancel,
                ..c.last
            }),
        })
    }
    pub fn remove_view(&mut self, view: u64) {
        if self.focused == Some(view) {
            self.focused = None;
        }
        self.captures.retain(|_, c| c.view != view);
        if self.focused.is_none() {
            self.blocked.extend(self.keys.keys().copied());
            self.keys.clear();
        }
    }
    pub fn pressed_views(&self) -> Vec<u64> {
        self.captures.values().map(|c| c.view).collect()
    }
}
