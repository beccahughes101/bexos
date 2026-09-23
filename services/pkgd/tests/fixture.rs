use bexos_pkg_client::{ArtifactKind, ArtifactQuery};
use bexos_pkg_config::{Config, Consumer, Repository};
use bexos_pkgd::{
    Error, Result,
    secure_state::{RepositoryState, SecureStore},
    transport::{Response, Transport},
};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    future::Future,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

pub fn hex(bytes: &[u8]) -> String {
    bexos_pkgd::oci::hex(bytes)
}
pub fn signed(value: Value) -> Vec<u8> {
    let bytes = serde_json::to_vec(&value).unwrap();
    let key = SigningKey::from_bytes(&[42; 32]);
    serde_json::to_vec(&json!({"signed": value, "signatures": [{"keyid": "fixture", "sig": hex(&key.sign(&bytes).to_bytes())}]})).unwrap()
}
pub fn reference(bytes: &[u8]) -> Value {
    json!({"version": 1, "length": bytes.len(), "hashes": {"sha256": hex(&Sha256::digest(bytes))}})
}
pub struct Fixture {
    pub config: Config,
    pub query: ArtifactQuery,
    pub transport: MemoryTransport,
    pub payload: Vec<u8>,
}
pub fn fixture() -> Fixture {
    let key = SigningKey::from_bytes(&[42; 32]);
    let root = signed(
        json!({"_type": "root", "spec_version": "1.0.26", "version": 1, "expires": "2099-01-01T00:00:00Z", "keys": {"fixture": {"keytype":"ed25519", "scheme":"ed25519", "keyval":{"public":hex(&key.verifying_key().to_bytes())}}}, "roles": {"root":{"keyids":["fixture"],"threshold":1},"timestamp":{"keyids":["fixture"],"threshold":1},"snapshot":{"keyids":["fixture"],"threshold":1},"targets":{"keyids":["fixture"],"threshold":1}}}),
    );
    let payload = b"verified artifact".to_vec();
    let targets = signed(
        json!({"_type":"targets","spec_version":"1.0.26","version":1,"expires":"2099-01-01T00:00:00Z","targets":{"latest":{"length":payload.len(),"hashes":{"sha256":hex(&Sha256::digest(&payload))},"custom":{"bexos":{"kind":"application"}}}}}),
    );
    let snapshot = signed(
        json!({"_type":"snapshot","spec_version":"1.0.26","version":1,"expires":"2099-01-01T00:00:00Z","meta":{"targets.json":reference(&targets)}}),
    );
    let timestamp = signed(
        json!({"_type":"timestamp","spec_version":"1.0.26","version":1,"expires":"2099-01-01T00:00:00Z","meta":{"snapshot.json":reference(&snapshot)}}),
    );
    let manifest = serde_json::to_vec(&json!({"schemaVersion":2,"config":{"mediaType":"application/vnd.oci.empty.v1+json","size":2,"digest":format!("sha256:{}",hex(&Sha256::digest(b"{}")))},"mediaType":"application/vnd.oci.image.manifest.v1+json","artifactType":"application/vnd.bexos.tuf.role.v1","layers":[{"mediaType":"application/vnd.bexos.tuf.timestamp.v1+json","digest":format!("sha256:{}",hex(&Sha256::digest(&timestamp))),"size":timestamp.len()}]})).unwrap();
    let mut responses = BTreeMap::new();
    responses.insert("/v2/apps/demo/manifests/tuf-timestamp".into(), manifest);
    for bytes in [&payload, &targets, &snapshot, &timestamp] {
        responses.insert(
            format!("/v2/apps/demo/blobs/sha256:{}", hex(&Sha256::digest(bytes))),
            bytes.clone(),
        );
    }
    let repo = Repository {
        host: "registry.test".into(),
        repository: "apps/demo".into(),
        trusted_root: root,
        token_origins: Vec::new(),
        redirect_origins: Vec::new(),
        tls_roots_der: Vec::new(),
    };
    let config = Config {
        repositories: vec![repo.clone()],
        consumers: vec![Consumer {
            package: "appd".into(),
            kinds: vec![1],
            repositories: vec![repo.id()],
        }],
        mappings: Vec::new(),
        max_cache_bytes: 1024 * 1024,
        max_payload_bytes: 1024,
        max_inflight: 4,
        max_waiters: 16,
        connect_timeout_ms: 10_000,
        request_timeout_ms: 30_000,
    };
    Fixture {
        config,
        query: ArtifactQuery {
            registry_host: repo.host,
            repository: repo.repository,
            tag: "latest".into(),
            expected_digest: None,
            kind: ArtifactKind::Application,
        },
        transport: MemoryTransport {
            responses,
            calls: Vec::new(),
        },
        payload,
    }
}
#[derive(Clone)]
pub struct MemoryTransport {
    pub responses: BTreeMap<String, Vec<u8>>,
    pub calls: Vec<String>,
}
impl Transport for MemoryTransport {
    async fn get(
        &mut self,
        _: &str,
        path: &str,
        _: &[(String, String)],
        maximum: usize,
    ) -> Result<Response> {
        self.calls.push(path.into());
        match self.responses.get(path) {
            Some(bytes) if bytes.len() <= maximum => Ok(Response {
                status: 200,
                headers: Vec::new(),
                body: bytes.clone(),
            }),
            Some(_) => Err(Error::ResourceExhausted),
            None => Ok(Response {
                status: 404,
                headers: Vec::new(),
                body: Vec::new(),
            }),
        }
    }
}
#[derive(Default)]
pub struct TestStore {
    pub state: Option<RepositoryState>,
    pub reject: bool,
    pub commits: usize,
}
impl SecureStore for TestStore {
    fn load(&mut self, _: &str) -> Result<Option<RepositoryState>> {
        Ok(self.state.clone())
    }
    fn commit(&mut self, _: &str, expected: u64, state: &RepositoryState) -> Result<()> {
        if self.reject {
            return Err(Error::Unavailable);
        }
        if self.state.as_ref().map_or(0, |state| state.revision) != expected {
            return Err(Error::VerifyFailed);
        }
        if let Some(old) = &self.state {
            for (name, floor) in old.floors() {
                if state.tuf.root.version > old.tuf.root.version
                    && (name == "$timestamp" || name == "$snapshot")
                {
                    continue;
                }
                if state.floors().get(&name).copied().unwrap_or(0) < floor {
                    return Err(Error::VerifyFailed);
                }
            }
        }
        self.commits += 1;
        self.state = Some(state.clone());
        Ok(())
    }
}
pub fn run<T>(future: impl Future<Output = T>) -> T {
    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }
    let waker = Waker::from(Arc::new(Noop));
    let mut context = Context::from_waker(&waker);
    let mut future = core::pin::pin!(future);
    for _ in 0..1000 {
        if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
            return result;
        }
    }
    panic!("fixture future did not complete");
}
