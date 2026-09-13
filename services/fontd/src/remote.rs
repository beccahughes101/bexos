//! Remote payloads are forwarded as pkgd-owned immutable VMOs. Fontd never
//! upgrades their rights or opens a network connection.
use crate::{
    index::{Index, Query, Scope},
    runtime::Runtime,
};
use alloc::{collections::BTreeMap, string::String, vec::Vec};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_pkg_client::{ArtifactKind, BlobDigest, HashType, PendingResolution};
use bexos_userspace::Channel;
use fonts_fidl::{FontHandle, FontStatus, HandleRef, WireVector};
#[derive(Default)]
pub struct Remote {
    pub endpoint: u64,
    pub directory: u64,
    pub known: BTreeMap<String, [u8; 32]>,
    pub requests: Vec<Request>,
    pub sent: bool,
}
pub struct Request {
    pub caller: u64,
    pub uid: u64,
    pub query: Query,
    pub allow: bool,
}
pub fn enqueue(
    runtime: &mut Runtime,
    caller: u64,
    uid: u64,
    query: &Query,
    allow: bool,
) -> Result<(), FontStatus> {
    let family = crate::index::normalize_family(&query.family).ok_or(FontStatus::InvalidArgs)?;
    if !runtime.remote.known.contains_key(&family) {
        if !allow {
            return Err(FontStatus::NotFound);
        }
        let config = bexos_pkg_config::Config::decode(include_bytes!(env!("PKGD_CONFIG")))
            .map_err(|_| FontStatus::Storage)?;
        if !config.mappings.iter().any(|mapping| {
            mapping.query.kind == ArtifactKind::Font
                && crate::index::normalize_family(&mapping.name).as_deref() == Some(&family)
        }) {
            return Err(FontStatus::NotFound);
        }
    }
    if runtime.remote.requests.len() >= 32 {
        return Err(FontStatus::Busy);
    }
    if runtime.remote.endpoint == 0 && runtime.remote.directory != 0 {
        runtime.remote.endpoint = bexos_userspace::service_directory::ServiceDirectoryClient::new(
            Channel(runtime.remote.directory),
        )
        .connect("bexos.pkg.PackageResolver", "Resolve")
        .map_err(|_| FontStatus::NotFound)?
        .0;
    }
    if runtime.remote.endpoint == 0 {
        return Err(FontStatus::NotFound);
    }
    runtime.remote.requests.push(Request {
        caller,
        uid,
        query: query.clone(),
        allow,
    });
    Ok(())
}
pub fn poll(runtime: &mut Runtime) -> bool {
    if runtime.remote.requests.is_empty() || runtime.remote.endpoint == 0 {
        return false;
    }
    let abandoned = !runtime.clients.iter().any(|client| {
        client.channel == runtime.remote.requests[0].caller
            && client.uid == runtime.remote.requests[0].uid
    });
    if abandoned {
        if runtime.remote.sent {
            let _ = bexos_userspace::Memory::close(runtime.remote.endpoint);
            runtime.remote.endpoint = 0;
            runtime.remote.sent = false;
            fail_all(runtime, FontStatus::NotFound);
        } else {
            runtime.remote.requests.remove(0);
        }
        return true;
    }
    let query = runtime.remote.requests[0].query.clone();
    let family = crate::index::normalize_family(&query.family).unwrap();
    if !runtime.remote.sent {
        let result = if let Some(digest) = runtime.remote.known.get(&family) {
            PendingResolution::begin_blob(
                Channel(runtime.remote.endpoint),
                BlobDigest {
                    hash_type: HashType::Sha256,
                    digest: *digest,
                },
            )
        } else {
            let config = match bexos_pkg_config::Config::decode(include_bytes!(env!("PKGD_CONFIG")))
            {
                Ok(config) => config,
                Err(_) => return false,
            };
            let Some(mapping) = config.mappings.iter().find(|mapping| {
                mapping.query.kind == ArtifactKind::Font
                    && crate::index::normalize_family(&mapping.name).as_deref() == Some(&family)
            }) else {
                return false;
            };
            PendingResolution::begin(Channel(runtime.remote.endpoint), &mapping.query)
        };
        match result {
            Ok(pending) => {
                runtime.remote.endpoint = pending.into_channel().unwrap().0;
                runtime.remote.sent = true;
            }
            Err(_) => {
                fail_all(runtime, FontStatus::Storage);
                runtime.remote.endpoint = 0;
                return true;
            }
        }
    }
    let expected = runtime.remote.known.get(&family).map(|digest| BlobDigest {
        hash_type: HashType::Sha256,
        digest: *digest,
    });
    let mut pending = PendingResolution::adopt(Channel(runtime.remote.endpoint), expected);
    let result = pending.poll();
    runtime.remote.endpoint = pending.into_channel().unwrap().0;
    if matches!(result, Err(bexos_pkg_client::PackageStatus::NotFound))
        && runtime.remote.requests[0].allow
        && runtime.remote.known.remove(&family).is_some()
    {
        runtime.remote.sent = false;
        return true;
    }
    match result {
        Ok(None) => false,
        result => {
            runtime.remote.sent = false;
            let request = runtime.remote.requests.remove(0);
            let resolved = result.map_err(|_| FontStatus::NotFound).and_then(|vmo| {
                let vmo = vmo.ok_or(FontStatus::NotFound)?;
                runtime.check_user(request.uid)?;
                let metadata =
                    crate::parser::parse(vmo.bytes()).map_err(crate::runtime::parser_status)?;
                let digest = crate::index::digest(vmo.bytes());
                let mut index = Index::default();
                index
                    .insert(Scope::System, digest, &metadata)
                    .map_err(|_| FontStatus::ResourceExhausted)?;
                let face = index
                    .resolve(request.uid, &request.query)
                    .ok_or(FontStatus::NotFound)?;
                if runtime.remote.known.len() < 128 {
                    runtime.remote.known.insert(family, digest);
                }
                Ok(FontHandle {
                    font_id: face.id,
                    data_len: vmo.length(),
                    index_in_collection: face.metadata.index,
                    data: HandleRef {
                        raw: vmo.into_handle(),
                    },
                })
            });
            answer(request.caller, resolved);
            true
        }
    }
}
fn answer(caller: u64, result: Result<FontHandle, FontStatus>) {
    let (status, fonts) = match result {
        Ok(font) => (FontStatus::Ok, alloc::vec![font]),
        Err(status) => (status, Vec::new()),
    };
    crate::wire::send_response(
        Channel(caller),
        &fonts_fidl::FontProviderResolveFontResponse {
            status,
            font: WireVector::from_slice(&fonts),
        },
    );
}
fn fail_all(runtime: &mut Runtime, status: FontStatus) {
    for request in runtime.remote.requests.drain(..) {
        answer(request.caller, Err(status));
    }
}
impl Remote {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(1);
        w.word(self.endpoint);
        w.word(self.directory);
        w.word(self.sent as u64);
        w.word(self.known.len() as u64);
        for (family, digest) in &self.known {
            w.text(family);
            w.bytes(digest);
        }
        w.word(self.requests.len() as u64);
        for request in &self.requests {
            w.word(request.caller);
            w.word(request.uid);
            w.word(request.allow as u64);
            w.text(&request.query.family);
            w.word(request.query.weight as u64);
            w.word(request.query.style as u64);
            w.word(request.query.format as u64);
        }
        w.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let endpoint = r.word()?;
        let directory = r.word()?;
        let sent = r.flag()?;
        let mut known = BTreeMap::new();
        for _ in 0..r.count(128)? {
            known.insert(
                r.text(64)?.into(),
                r.bytes(32)?.try_into().map_err(|_| Error::InvalidData)?,
            );
        }
        let mut requests = Vec::new();
        for _ in 0..r.count(32)? {
            let caller = r.word()?;
            let uid = r.word()?;
            let allow = r.flag()?;
            let family = r.text(64)?.into();
            let weight = u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            let style = match r.word()? {
                1 => fonts_fidl::FontStyle::Normal,
                2 => fonts_fidl::FontStyle::Italic,
                3 => fonts_fidl::FontStyle::Oblique,
                _ => return Err(Error::InvalidData),
            };
            let format = match r.word()? {
                1 => fonts_fidl::FontFormat::Truetype,
                2 => fonts_fidl::FontFormat::Opentype,
                3 => fonts_fidl::FontFormat::Woff2,
                4 => fonts_fidl::FontFormat::Collection,
                _ => return Err(Error::InvalidData),
            };
            requests.push(Request {
                caller,
                uid,
                allow,
                query: Query {
                    family,
                    weight,
                    style,
                    format,
                },
            });
        }
        r.finish()?;
        if sent && (endpoint == 0 || requests.is_empty()) {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            endpoint,
            directory,
            known,
            requests,
            sent,
        })
    }
}
