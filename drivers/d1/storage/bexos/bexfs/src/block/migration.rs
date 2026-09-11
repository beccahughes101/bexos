use super::*;
impl<B> FidlBlockDevice<B> {
    pub fn checkpoint_backend<T>(&self, f: impl FnOnce(&B, u64) -> T) -> T {
        f(&self.backend.borrow(), *self.next_request_id.borrow())
    }
    pub fn adopt_backend(backend: B, info: BlockInfo, next_request_id: u64) -> Self {
        Self {
            backend: RefCell::new(backend),
            info,
            next_request_id: RefCell::new(next_request_id),
        }
    }
    pub fn activate_backend(&self, f: impl FnOnce(&mut B)) {
        f(&mut self.backend.borrow_mut());
    }
}
