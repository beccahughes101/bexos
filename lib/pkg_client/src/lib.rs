#![no_std]
extern crate alloc;
mod vmo;
use alloc::{string::String, vec, vec::Vec};
use bexos_userspace::{Channel, Memory, Message};
use core::{future::poll_fn, task::Poll};
pub use pkg_fidl::{ArtifactKind, BlobDigest, HashType, PackageStatus};
use pkg_fidl::{FidlDecode, FidlEncode, HandleRef, WireVector};
pub use vmo::ReadOnlyVmo;

#[derive(Clone, Debug, PartialEq)]
pub struct ArtifactQuery {
    pub registry_host: String,
    pub repository: String,
    pub tag: String,
    pub expected_digest: Option<BlobDigest>,
    pub kind: ArtifactKind,
}
impl ArtifactQuery {
    pub fn validate(&self) -> Result<(), PackageStatus> {
        if !valid_host(&self.registry_host)
            || !valid_repository(&self.repository)
            || self.tag.is_empty()
            || self.tag.len() > 64
            || self.tag.starts_with(['.', '-'])
            || !self
                .tag
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(PackageStatus::InvalidArgs);
        }
        Ok(())
    }
}
pub fn split_registry_authority(authority: &str) -> Option<(&str, u16)> {
    if authority.is_empty() || authority.len() > 128 {
        return None;
    }
    let (host, port) = if let Some((host, port)) = authority.split_once(':') {
        let parsed = port.parse::<u16>().ok()?;
        if parsed == 0 || port.starts_with('0') || !port.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        (host, parsed)
    } else {
        (authority, 443)
    };
    if !host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }) {
        return None;
    }
    Some((host, port))
}
pub fn valid_host(host: &str) -> bool {
    split_registry_authority(host).is_some()
}
pub fn valid_repository(repository: &str) -> bool {
    if repository.is_empty() || repository.len() > 128 {
        return false;
    }
    repository.split('/').all(|part| {
        let bytes = part.as_bytes();
        let mut at = 0;
        loop {
            let start = at;
            while at < bytes.len() && (bytes[at].is_ascii_lowercase() || bytes[at].is_ascii_digit())
            {
                at += 1;
            }
            if at == start {
                return false;
            }
            if at == bytes.len() {
                return true;
            }
            match bytes[at] {
                b'.' => at += 1,
                b'_' => {
                    at += 1;
                    if bytes.get(at) == Some(&b'_') {
                        at += 1;
                    }
                }
                b'-' => {
                    while bytes.get(at) == Some(&b'-') {
                        at += 1;
                    }
                }
                _ => return false,
            }
        }
    })
}

/// An owned resolver channel. Only one call is outstanding per channel.
/// Cancellation closes the channel so a delayed reply cannot be mistaken for a new one.
pub struct PackageClient {
    channel: Option<Channel>,
}
impl PackageClient {
    pub fn from_channel(channel: Channel) -> Self {
        Self {
            channel: Some(channel),
        }
    }
    pub fn into_channel(mut self) -> Option<Channel> {
        self.channel.take()
    }
    pub async fn fetch_artifact(
        &mut self,
        query: &ArtifactQuery,
    ) -> Result<ReadOnlyVmo, PackageStatus> {
        query.validate()?;
        let expected: Vec<_> = query.expected_digest.iter().copied().collect();
        let request = pkg_fidl::PackageResolverResolveArtifactRequest {
            query: pkg_fidl::ArtifactQuery {
                registry_host: &query.registry_host,
                repository: &query.repository,
                tag: &query.tag,
                expected_digest: WireVector::from_slice(&expected),
                kind: query.kind,
            },
        };
        let message = self.call(1, &request).await?;
        decode_blob(message, query.expected_digest.as_ref(), false)
    }
    pub async fn fetch_blob(&mut self, digest: BlobDigest) -> Result<ReadOnlyVmo, PackageStatus> {
        let message = self
            .call(
                2,
                &pkg_fidl::PackageResolverResolveBlobRequest {
                    digest,
                    allow_network_fetch: false,
                },
            )
            .await?;
        decode_blob(message, Some(&digest), true)
    }
    /// Fetch from a previously verified source when the authorized cache entry is absent.
    pub async fn fetch_blob_from_source(
        &mut self,
        digest: BlobDigest,
    ) -> Result<ReadOnlyVmo, PackageStatus> {
        let message = self
            .call(
                2,
                &pkg_fidl::PackageResolverResolveBlobRequest {
                    digest,
                    allow_network_fetch: true,
                },
            )
            .await?;
        decode_blob(message, Some(&digest), true)
    }
    async fn call(
        &mut self,
        ordinal: u64,
        request: &impl FidlEncode,
    ) -> Result<Message, PackageStatus> {
        let mut bytes = vec![0; 1024];
        let encoded = request
            .encode(&mut bytes[8..], &mut [])
            .map_err(|_| PackageStatus::InvalidArgs)?;
        let length = encoded.bytes + 8;
        bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
        let channel = self.channel.take().ok_or(PackageStatus::Unavailable)?;
        let mut pending = Pending(Some(channel));
        channel
            .send(&bytes[..length], &[])
            .map_err(|_| PackageStatus::Unavailable)?;
        let message = poll_fn(|cx| match channel.try_recv() {
            Err(kernel_fidl::Status::ErrTimedOut) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            result => Poll::Ready(result),
        })
        .await
        .map_err(|_| PackageStatus::Unavailable)?;
        self.channel = pending.0.take();
        Ok(message)
    }
}
impl Drop for PackageClient {
    fn drop(&mut self) {
        if let Some(channel) = self.channel.take() {
            let _ = Memory::close(channel.0);
        }
    }
}
struct Pending(Option<Channel>);
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(channel) = self.0 {
            let _ = Memory::close(channel.0);
        }
    }
}

/// A pollable call for service event loops and live-migration records.
pub struct PendingResolution {
    channel: Option<Channel>,
    expected: Option<BlobDigest>,
}
impl PendingResolution {
    pub fn begin_blob(channel: Channel, digest: BlobDigest) -> Result<Self, PackageStatus> {
        let pending = Self {
            channel: Some(channel),
            expected: Some(digest),
        };
        let mut bytes = [0; 128];
        let encoded = pkg_fidl::PackageResolverResolveBlobRequest {
            digest,
            allow_network_fetch: false,
        }
        .encode(&mut bytes[8..], &mut [])
        .map_err(|_| PackageStatus::InvalidArgs)?;
        let length = encoded.bytes + 8;
        bytes[..8].copy_from_slice(&2u64.to_le_bytes());
        channel
            .send(&bytes[..length], &[])
            .map_err(|_| PackageStatus::Unavailable)?;
        Ok(pending)
    }
    pub fn begin(channel: Channel, query: &ArtifactQuery) -> Result<Self, PackageStatus> {
        let pending = Self {
            channel: Some(channel),
            expected: query.expected_digest,
        };
        query.validate()?;
        let expected: Vec<_> = query.expected_digest.iter().copied().collect();
        let request = pkg_fidl::PackageResolverResolveArtifactRequest {
            query: pkg_fidl::ArtifactQuery {
                registry_host: &query.registry_host,
                repository: &query.repository,
                tag: &query.tag,
                expected_digest: WireVector::from_slice(&expected),
                kind: query.kind,
            },
        };
        let mut bytes = vec![0; 1024];
        let encoded = request
            .encode(&mut bytes[8..], &mut [])
            .map_err(|_| PackageStatus::InvalidArgs)?;
        let length = encoded.bytes + 8;
        bytes[..8].copy_from_slice(&1u64.to_le_bytes());
        channel
            .send(&bytes[..length], &[])
            .map_err(|_| PackageStatus::Unavailable)?;
        Ok(pending)
    }
    pub fn adopt(channel: Channel, expected: Option<BlobDigest>) -> Self {
        Self {
            channel: Some(channel),
            expected,
        }
    }
    pub fn channel(&self) -> Option<Channel> {
        self.channel
    }
    pub fn into_channel(mut self) -> Option<Channel> {
        self.channel.take()
    }
    pub fn poll(&mut self) -> Result<Option<ReadOnlyVmo>, PackageStatus> {
        let channel = self.channel.ok_or(PackageStatus::Unavailable)?;
        match channel.try_recv() {
            Err(kernel_fidl::Status::ErrTimedOut) => Ok(None),
            Err(_) => Err(PackageStatus::Unavailable),
            Ok(message) => decode_blob(message, self.expected.as_ref(), false).map(Some),
        }
    }
}
impl Drop for PendingResolution {
    fn drop(&mut self) {
        if let Some(channel) = self.channel.take() {
            let _ = Memory::close(channel.0);
        }
    }
}

fn decode_blob(
    message: Message,
    expected: Option<&BlobDigest>,
    raw: bool,
) -> Result<ReadOnlyVmo, PackageStatus> {
    let refs: Vec<_> = message
        .handles
        .iter()
        .map(|raw| HandleRef { raw: *raw })
        .collect();
    let result = (|| {
        let (status, blobs) = if raw {
            let response =
                pkg_fidl::PackageResolverResolveBlobResponse::decode(&message.bytes, &refs)
                    .map_err(|_| PackageStatus::VerifyFailed)?;
            (response.status, response.blob)
        } else {
            let response =
                pkg_fidl::PackageResolverResolveArtifactResponse::decode(&message.bytes, &refs)
                    .map_err(|_| PackageStatus::VerifyFailed)?;
            (response.status, response.blob)
        };
        if status != PackageStatus::Ok {
            return Err(status);
        }
        if blobs.len() != 1 || refs.len() != 1 {
            return Err(PackageStatus::VerifyFailed);
        }
        let blob = blobs.get(0).map_err(|_| PackageStatus::VerifyFailed)?;
        if blob.data.raw != message.handles[0] {
            return Err(PackageStatus::VerifyFailed);
        }
        Ok((blob.data.raw, blob.content_length, blob.verified_digest))
    })();
    match result {
        Ok((handle, length, digest)) => ReadOnlyVmo::adopt(handle, length, digest, expected),
        Err(status) => {
            for handle in message.handles {
                let _ = Memory::close(handle);
            }
            Err(status)
        }
    }
}
