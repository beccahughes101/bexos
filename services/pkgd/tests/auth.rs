use super::fixture::{fixture, run};
use bexos_pkgd::{
    Error, Result,
    credentials::Secret,
    oci::Oci,
    transport::{Response, Transport},
};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
#[derive(Clone)]
struct Script {
    responses: Rc<RefCell<VecDeque<Response>>>,
    calls: Rc<RefCell<Vec<(String, String, Vec<(String, String)>)>>>,
}
impl Transport for Script {
    async fn get(
        &mut self,
        host: &str,
        path: &str,
        headers: &[(String, String)],
        maximum: usize,
    ) -> Result<Response> {
        self.calls
            .borrow_mut()
            .push((host.into(), path.into(), headers.to_vec()));
        let response = self
            .responses
            .borrow_mut()
            .pop_front()
            .expect("unexpected network operation");
        if response.body.len() > maximum {
            return Err(Error::ResourceExhausted);
        }
        Ok(response)
    }
}
fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Response {
    Response {
        status,
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body: body.to_vec(),
    }
}
fn script(responses: Vec<Response>) -> Script {
    Script {
        responses: Rc::new(RefCell::new(responses.into())),
        calls: Default::default(),
    }
}
#[test]
fn redirects_never_receive_registry_bearer_tokens() {
    let mut repo = fixture().config.repositories.remove(0);
    repo.redirect_origins.push("https://cdn.test".into());
    let transport = script(vec![
        response(307, &[("location", "https://cdn.test/blob")], &[]),
        response(200, &[], b"verified artifact"),
    ]);
    let mut oci = Oci {
        repository: repo,
        transport: transport.clone(),
        token: Some(Secret::new(b"fixture-token".to_vec())),
        transfers: Default::default(),
        credential_generation: 1,
    };
    run(oci.blob(&Sha256::digest(b"verified artifact").into(), 17)).unwrap();
    let calls = transport.calls.borrow();
    assert!(calls[0].2.iter().any(|(k, _)| k == "Authorization"));
    assert!(calls[1].2.iter().all(|(k, _)| k != "Authorization"));
}
#[test]
fn bearer_exchange_is_allowlisted_scoped_and_does_not_forward_registry_credentials() {
    let mut repo = fixture().config.repositories.remove(0);
    repo.token_origins.push("https://issuer.test".into());
    let transport = script(vec![
        response(
            401,
            &[(
                "www-authenticate",
                "bEaReR realm=\"https://issuer.test/token\",service=\"registry.test\",scope=\"repository:apps/demo:pull\"",
            )],
            &[],
        ),
        response(200, &[], br#"{"token":"issued"}"#),
        response(200, &[], b"verified artifact"),
    ]);
    let mut oci = Oci {
        repository: repo,
        transport: transport.clone(),
        token: Some(Secret::new(b"old".to_vec())),
        transfers: Default::default(),
        credential_generation: 1,
    };
    run(oci.blob(&Sha256::digest(b"verified artifact").into(), 17)).unwrap();
    let calls = transport.calls.borrow();
    assert_eq!(calls[1].0, "issuer.test");
    assert!(calls[1].1.contains("scope=repository%3Aapps%2Fdemo%3Apull"));
    assert!(calls[1].2.is_empty());
    assert!(
        calls[2]
            .2
            .iter()
            .any(|(k, v)| k == "Authorization" && v == "Bearer issued")
    );
}
#[test]
fn untrusted_token_issuers_and_redirects_are_rejected_before_contact() {
    for response in [
        response(302, &[("location", "https://evil.test/blob")], &[]),
        response(
            401,
            &[(
                "www-authenticate",
                "Bearer realm=\"https://evil.test/token\"",
            )],
            &[],
        ),
    ] {
        let transport = script(vec![response]);
        let mut oci = Oci {
            repository: fixture().config.repositories.remove(0),
            transport: transport.clone(),
            token: None,
            transfers: Default::default(),
            credential_generation: 0,
        };
        assert!(matches!(
            run(oci.blob(&Sha256::digest(b"verified artifact").into(), 17)),
            Err(Error::AccessDenied)
        ));
        assert_eq!(transport.calls.borrow().len(), 1);
    }
}

#[test]
fn relative_redirects_keep_origin_and_authentication() {
    let transport = script(vec![
        response(307, &[("location", "/relocated/blob")], &[]),
        response(200, &[], b"verified artifact"),
    ]);
    let mut oci = Oci {
        repository: fixture().config.repositories.remove(0),
        transport: transport.clone(),
        token: Some(Secret::new(b"fixture-token".to_vec())),
        transfers: Default::default(),
        credential_generation: 1,
    };
    run(oci.blob(&Sha256::digest(b"verified artifact").into(), 17)).unwrap();
    let calls = transport.calls.borrow();
    assert_eq!(calls[0].0, calls[1].0);
    assert_eq!(calls[1].1, "/relocated/blob");
    assert!(
        calls[1]
            .2
            .iter()
            .any(|(k, v)| k == "Authorization" && v == "Bearer fixture-token")
    );
}
