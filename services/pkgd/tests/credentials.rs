use super::fixture::{fixture, run};
use bexos_pkgd::{
    Error, Result,
    credentials::{Secret, Vault},
    oci::Oci,
    transfers::Transfers,
    transport::{Response, Transport},
};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    rc::Rc,
    task::{Context, Poll, Waker},
};

#[derive(Clone, Default)]
struct HeldTransport {
    ready: Rc<Cell<bool>>,
    active: Rc<Cell<usize>>,
    credentials: Rc<RefCell<Vec<String>>>,
}
struct Connection(Rc<Cell<usize>>);
impl Drop for Connection {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}
impl Transport for HeldTransport {
    async fn get(
        &mut self,
        _: &str,
        _: &str,
        headers: &[(String, String)],
        maximum: usize,
    ) -> Result<Response> {
        self.active.set(self.active.get() + 1);
        let _connection = Connection(self.active.clone());
        self.credentials.borrow_mut().push(
            headers
                .iter()
                .find(|(name, _)| name == "Authorization")
                .unwrap()
                .1
                .clone(),
        );
        poll_fn(|_| {
            if self.ready.get() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        assert!(maximum >= b"verified artifact".len());
        Ok(Response {
            status: 200,
            headers: vec![],
            body: b"verified artifact".to_vec(),
        })
    }
}

#[test]
fn replacement_credentials_cannot_join_an_old_authenticated_transfer() {
    let repository = fixture().config.repositories.remove(0);
    let pool = Transfers::new(2);
    let transport = HeldTransport::default();
    let mut vault = Vault::default();
    vault
        .set_token("registry.test", b"old-token".to_vec(), false)
        .unwrap();
    let make_oci = |vault: &Vault| {
        let credential = vault.get("registry.test").unwrap();
        Oci {
            repository: repository.clone(),
            transport: transport.clone(),
            token: Some(Secret::new(credential.token.bytes().to_vec())),
            transfers: pool.clone(),
            credential_generation: credential.generation,
        }
    };
    let digest: [u8; 32] = Sha256::digest(b"verified artifact").into();
    let mut old = make_oci(&vault);
    let mut old_read = Box::pin(old.payload(&digest, 17));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(old_read.as_mut().poll(&mut cx).is_pending());
    vault
        .set_token("registry.test", b"new-token".to_vec(), false)
        .unwrap();
    let mut new = make_oci(&vault);
    let mut new_read = Box::pin(new.payload(&digest, 17));
    assert!(new_read.as_mut().poll(&mut cx).is_pending());
    assert_eq!(
        &*transport.credentials.borrow(),
        &["Bearer old-token", "Bearer new-token"]
    );
    assert_eq!(transport.active.get(), 2);
    drop(old_read);
    assert_eq!(transport.active.get(), 1);
    transport.ready.set(true);
    assert_eq!(&**run(new_read).unwrap(), b"verified artifact");
    assert_eq!(transport.active.get(), 0);
}

#[test]
fn credential_removal_and_reprovisioning_never_reuse_a_transfer_generation() {
    let mut vault = Vault::default();
    vault
        .set_token("registry.test", b"first".to_vec(), true)
        .unwrap();
    let first = vault.get("registry.test").unwrap().generation;
    assert_eq!(
        vault.set_token("registry.test", b"bad\ntoken".to_vec(), true),
        Err(Error::InvalidArgs)
    );
    assert_eq!(vault.get("registry.test").unwrap().generation, first);
    assert_eq!(vault.get("registry.test").unwrap().token.bytes(), b"first");
    vault.remove("registry.test");
    assert!(vault.get("registry.test").is_none());
    vault
        .set_token("registry.test", b"second".to_vec(), true)
        .unwrap();
    assert!(vault.get("registry.test").unwrap().generation > first);
}
