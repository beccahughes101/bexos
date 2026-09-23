//! Shared digest transfers. Weak table entries do not keep abandoned I/O alive.
use crate::{Error, Result, payload::Payload};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    future::{Future, poll_fn},
    pin::Pin,
    rc::{Rc, Weak},
    task::Poll,
};
type Work = Pin<Box<dyn Future<Output = Result<Payload>>>>;
struct Flight {
    work: Option<Work>,
    result: Option<Result<Rc<Payload>>>,
}
#[derive(Clone)]
pub struct Transfers {
    entries: Rc<RefCell<BTreeMap<String, Weak<RefCell<Flight>>>>>,
    maximum: usize,
}
impl Default for Transfers {
    fn default() -> Self {
        Self::new(16)
    }
}
impl Transfers {
    pub fn new(maximum: usize) -> Self {
        Self {
            entries: Default::default(),
            maximum,
        }
    }
    pub async fn get(&self, key: String, create: impl FnOnce() -> Work) -> Result<Rc<Payload>> {
        let flight = {
            let mut entries = self.entries.borrow_mut();
            entries.retain(|_, entry| entry.strong_count() != 0);
            if let Some(flight) = entries.get(&key).and_then(Weak::upgrade) {
                flight
            } else {
                if entries.len() >= self.maximum {
                    return Err(Error::ResourceExhausted);
                }
                let flight = Rc::new(RefCell::new(Flight {
                    work: Some(create()),
                    result: None,
                }));
                entries.insert(key, Rc::downgrade(&flight));
                flight
            }
        };
        poll_fn(|cx| {
            let mut flight = flight.borrow_mut();
            if let Some(result) = &flight.result {
                return Poll::Ready(result.clone());
            }
            match flight.work.as_mut().unwrap().as_mut().poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(result) => {
                    let result = result.map(Rc::new);
                    flight.work = None;
                    flight.result = Some(result.clone());
                    Poll::Ready(result)
                }
            }
        })
        .await
    }
}
