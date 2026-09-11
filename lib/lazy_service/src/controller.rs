use alloc::rc::Rc;
use core::cell::RefCell;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdleDecision {
    Busy,
    Deadline { generation: u64, deadline_ns: u64 },
    Ready { generation: u64 },
    Stopping,
    Suspended,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopDecision {
    Accepted { generation: u64 },
    Busy,
    Stale,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LazyServiceSnapshot {
    pub generation: u64,
    pub connection_count: u32,
    pub keep_alive_count: u32,
    pub idle_timeout_ns: u64,
    pub idle_deadline_ns: Option<u64>,
    pub stopping: bool,
    pub suspended: bool,
}

#[derive(Debug, Default)]
struct Inner {
    generation: u64,
    connection_count: u32,
    keep_alive_count: u32,
    idle_timeout_ns: u64,
    idle_deadline_ns: Option<u64>,
    stopping: bool,
    suspended_count: u32,
}

#[derive(Clone, Debug)]
pub struct LazyServiceController {
    inner: Rc<RefCell<Inner>>,
}

impl LazyServiceController {
    pub fn new(idle_timeout_ms: u32) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                idle_timeout_ns: u64::from(idle_timeout_ms) * 1_000_000,
                ..Inner::default()
            })),
        }
    }

    pub fn restore(snapshot: LazyServiceSnapshot) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                generation: snapshot.generation,
                connection_count: snapshot.connection_count,
                keep_alive_count: snapshot.keep_alive_count,
                idle_timeout_ns: snapshot.idle_timeout_ns,
                idle_deadline_ns: snapshot.idle_deadline_ns,
                stopping: snapshot.stopping,
                suspended_count: snapshot.suspended as u32,
            })),
        }
    }

    pub fn snapshot(&self) -> LazyServiceSnapshot {
        let inner = self.inner.borrow();
        LazyServiceSnapshot {
            generation: inner.generation,
            connection_count: inner.connection_count,
            keep_alive_count: inner.keep_alive_count,
            idle_timeout_ns: inner.idle_timeout_ns,
            idle_deadline_ns: inner.idle_deadline_ns,
            stopping: inner.stopping,
            suspended: inner.suspended_count != 0,
        }
    }

    pub fn track_connection(&self) -> ConnectionGuard {
        let mut inner = self.inner.borrow_mut();
        inner.connection_count = inner.connection_count.saturating_add(1);
        inner.idle_deadline_ns = None;
        ConnectionGuard {
            inner: Some(self.inner.clone()),
        }
    }

    pub fn keep_alive(&self) -> KeepAlive {
        let mut inner = self.inner.borrow_mut();
        inner.keep_alive_count = inner.keep_alive_count.saturating_add(1);
        inner.idle_deadline_ns = None;
        KeepAlive {
            inner: Some(self.inner.clone()),
            kind: KeepAliveKind::Activity,
        }
    }

    pub fn suspend_idle(&self) -> KeepAlive {
        let mut inner = self.inner.borrow_mut();
        inner.suspended_count = inner.suspended_count.saturating_add(1);
        inner.idle_deadline_ns = None;
        KeepAlive {
            inner: Some(self.inner.clone()),
            kind: KeepAliveKind::Suspension,
        }
    }

    pub fn restore_connection_guard(&self) -> ConnectionGuard {
        ConnectionGuard {
            inner: Some(self.inner.clone()),
        }
    }

    pub fn restore_keep_alive(&self) -> KeepAlive {
        KeepAlive {
            inner: Some(self.inner.clone()),
            kind: KeepAliveKind::Activity,
        }
    }

    pub fn idle_decision(&self, now_ns: u64) -> IdleDecision {
        let mut inner = self.inner.borrow_mut();
        if inner.stopping {
            return IdleDecision::Stopping;
        }
        if inner.suspended_count != 0 {
            inner.idle_deadline_ns = None;
            return IdleDecision::Suspended;
        }
        if inner.connection_count != 0 || inner.keep_alive_count != 0 {
            inner.idle_deadline_ns = None;
            return IdleDecision::Busy;
        }
        let generation = inner.generation;
        let deadline = match inner.idle_deadline_ns {
            Some(deadline) => deadline,
            None => {
                let deadline = now_ns.saturating_add(inner.idle_timeout_ns);
                inner.idle_deadline_ns = Some(deadline);
                deadline
            }
        };
        if now_ns >= deadline {
            IdleDecision::Ready { generation }
        } else {
            IdleDecision::Deadline {
                generation,
                deadline_ns: deadline,
            }
        }
    }

    pub fn acknowledge_stop(&self, generation: u64) -> StopDecision {
        let mut inner = self.inner.borrow_mut();
        if generation != inner.generation {
            return StopDecision::Stale;
        }
        if inner.connection_count != 0 || inner.keep_alive_count != 0 || inner.suspended_count != 0
        {
            inner.idle_deadline_ns = None;
            return StopDecision::Busy;
        }
        inner.stopping = true;
        inner.generation = inner.generation.saturating_add(1);
        StopDecision::Accepted {
            generation: inner.generation,
        }
    }

    pub fn cancel_stop(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.stopping = false;
        inner.idle_deadline_ns = None;
    }
}

pub struct ConnectionGuard {
    inner: Option<Rc<RefCell<Inner>>>,
}

impl ConnectionGuard {
    pub fn release(mut self) {
        self.drop_inner();
    }

    fn drop_inner(&mut self) {
        if let Some(inner) = self.inner.take() {
            let mut inner = inner.borrow_mut();
            inner.connection_count = inner.connection_count.saturating_sub(1);
            inner.idle_deadline_ns = None;
        }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.drop_inner();
    }
}

pub struct KeepAlive {
    inner: Option<Rc<RefCell<Inner>>>,
    kind: KeepAliveKind,
}

#[derive(Clone, Copy)]
enum KeepAliveKind {
    Activity,
    Suspension,
}

impl KeepAlive {
    pub fn release(mut self) {
        self.drop_inner();
    }

    fn drop_inner(&mut self) {
        if let Some(inner) = self.inner.take() {
            let mut inner = inner.borrow_mut();
            match self.kind {
                KeepAliveKind::Activity => {
                    inner.keep_alive_count = inner.keep_alive_count.saturating_sub(1);
                }
                KeepAliveKind::Suspension => {
                    inner.suspended_count = inner.suspended_count.saturating_sub(1);
                }
            }
            inner.idle_deadline_ns = None;
        }
    }
}

impl Drop for KeepAlive {
    fn drop(&mut self) {
        self.drop_inner();
    }
}
