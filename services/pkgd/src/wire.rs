use crate::{
    Error, Result,
    runtime::{Client, Job, Runtime, SecretHandle},
};
use bexos_pkg_client::{ArtifactQuery, BlobDigest};
use bexos_userspace::{Channel, Memory};
use pkg_fidl::{FidlDecode, FidlEncode, HandleRef, WireVector};
use sha2::{Digest, Sha256};

pub fn poll(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    let clients = core::mem::take(&mut runtime.clients);
    for client in clients {
        match Channel(client.channel).try_recv() {
            Ok(mut message) => {
                changed = true;
                let ordinal = message
                    .bytes
                    .get(..8)
                    .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
                    .unwrap_or(0);
                if client.methods.contains(&ordinal) && message.bytes.len() >= 8 {
                    handle(
                        runtime,
                        &client,
                        ordinal,
                        &message.bytes[8..],
                        &message.handles,
                    );
                } else if client.protocol == "PackageResolver" {
                    reply(runtime, client.channel, Err(Error::AccessDenied));
                } else {
                    credential_reply(client.channel, Err(Error::AccessDenied));
                }
                for handle in message.handles {
                    let _ = Memory::close(handle);
                }
                if client.protocol == "CredentialManager" {
                    for byte in &mut message.bytes {
                        unsafe {
                            core::ptr::write_volatile(byte, 0);
                        }
                    }
                }
                runtime.clients.push(client);
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                changed = true;
                let _ = Memory::close(client.channel);
            }
            Err(_) => runtime.clients.push(client),
        }
    }
    changed
}

fn handle(runtime: &mut Runtime, client: &Client, ordinal: u64, bytes: &[u8], handles: &[u64]) {
    let refs: Vec<_> = handles.iter().map(|raw| HandleRef { raw: *raw }).collect();
    if client.protocol == "CredentialManager" {
        let result = match ordinal {
            1 if handles.is_empty() => {
                pkg_fidl::CredentialManagerSetRegistryCredentialRequest::decode(bytes, &refs)
                    .map_err(|_| Error::InvalidArgs)
                    .and_then(|request| {
                        set_token(
                            runtime,
                            request.registry_host,
                            request.auth_token.as_bytes(),
                            request.sealed_in_trusty,
                        )
                    })
            }
            2 if handles.len() == 2 => {
                pkg_fidl::CredentialManagerSetRegistryMtlsIdentityRequest::decode(bytes, &refs)
                    .map_err(|_| Error::InvalidArgs)
                    .and_then(|request| {
                        let certificates = read_vmo(
                            request.certificate_chain.raw,
                            request.certificate_chain_length,
                            16 * 1024,
                        )?;
                        let private_key =
                            read_vmo(request.private_key.raw, request.private_key_length, 3072)?;
                        set_identity(
                            runtime,
                            request.registry_host,
                            certificates,
                            private_key,
                            request.sealed_in_trusty,
                        )
                    })
            }
            3 if handles.is_empty() => {
                pkg_fidl::CredentialManagerRemoveRegistryCredentialRequest::decode(bytes, &refs)
                    .map_err(|_| Error::InvalidArgs)
                    .and_then(|request| {
                        validate_registry(runtime, request.registry_host)?;
                        for suffix in ["token", "key"] {
                            sealed_write(
                                runtime,
                                &format!("credential:{}:{suffix}", request.registry_host),
                                &[],
                            )?;
                        }
                        runtime.vault.remove(request.registry_host);
                        invalidate(runtime, request.registry_host);
                        if let Some(secret) = runtime.secrets.remove(request.registry_host) {
                            let _ = Memory::close(secret.handle);
                        }
                        Ok(())
                    })
            }
            _ => Err(Error::InvalidArgs),
        };
        credential_reply(client.channel, result);
        return;
    }
    if !handles.is_empty() {
        reply(runtime, client.channel, Err(Error::InvalidArgs));
        return;
    }
    match ordinal {
        1 => {
            let result = pkg_fidl::PackageResolverResolveArtifactRequest::decode(bytes, &[])
                .map_err(|_| Error::InvalidArgs)
                .and_then(|request| {
                    let expected_digest = if request.query.expected_digest.len() == 0 {
                        None
                    } else {
                        Some(
                            request
                                .query
                                .expected_digest
                                .get(0)
                                .map_err(|_| Error::InvalidArgs)?,
                        )
                    };
                    runtime.enqueue(
                        client,
                        ArtifactQuery {
                            registry_host: request.query.registry_host.into(),
                            repository: request.query.repository.into(),
                            tag: request.query.tag.into(),
                            expected_digest,
                            kind: request.query.kind,
                        },
                    )
                });
            if let Err(error) = result {
                reply(runtime, client.channel, Err(error));
            }
        }
        2 => {
            let request = match pkg_fidl::PackageResolverResolveBlobRequest::decode(bytes, &[]) {
                Ok(request) => request,
                Err(_) => {
                    reply(runtime, client.channel, Err(Error::InvalidArgs));
                    return;
                }
            };
            let result = runtime.cached(&client.package, &request.digest);
            if result == Err(Error::NotFound) && request.allow_network_fetch {
                let source = runtime
                    .secure
                    .borrow_mut()
                    .as_mut()
                    .ok_or(Error::Unavailable)
                    .and_then(|secure| {
                        crate::resolution::known_source(
                            &runtime.config,
                            &client.package,
                            &request.digest,
                            secure,
                        )
                    });
                match source
                    .and_then(|query| runtime.enqueue_source(client, query, Some(request.digest)))
                {
                    Ok(()) => {}
                    Err(error) => reply(runtime, client.channel, Err(error)),
                }
            } else {
                reply(runtime, client.channel, result);
            }
        }
        _ => reply(runtime, client.channel, Err(Error::InvalidArgs)),
    }
}
pub fn finish(runtime: &Runtime, job: &Job, result: Result<BlobDigest>) {
    for waiter in &job.waiters {
        let result = result.and_then(|digest| {
            if !runtime.config.repository(&job.query).is_some_and(|repo| {
                runtime
                    .config
                    .permits(&waiter.caller, &repo.id(), job.query.kind)
            }) {
                return Err(Error::AccessDenied);
            }
            if let Some(expected) = waiter.expected {
                let blob = runtime.blobs.get(&digest.digest).ok_or(Error::NotFound)?;
                let address = Memory::map(blob.handle, blob.length, kernel_fidl::Rights::READ.0)
                    .map_err(|_| Error::Io)?;
                let bytes = unsafe {
                    core::slice::from_raw_parts(address as *const u8, blob.length as usize)
                };
                let hash: [u8; 32] = match expected.hash_type {
                    pkg_fidl::HashType::Sha256 => Sha256::digest(bytes).into(),
                    pkg_fidl::HashType::Blake3 => *blake3::hash(bytes).as_bytes(),
                };
                Memory::unmap(address, blob.length).map_err(|_| Error::Io)?;
                if hash != expected.digest {
                    return Err(Error::VerifyFailed);
                }
            }
            Ok(digest)
        });
        reply(runtime, waiter.channel, result);
    }
}
fn reply(runtime: &Runtime, channel: u64, result: Result<BlobDigest>) {
    let result = result.and_then(|digest| {
        let blob = runtime.blobs.get(&digest.digest).ok_or(Error::NotFound)?;
        let rights = kernel_fidl::Rights::TRANSFER.0
            | kernel_fidl::Rights::READ.0
            | kernel_fidl::Rights::MAP.0;
        let handle =
            Memory::duplicate(blob.handle, rights).map_err(|_| Error::ResourceExhausted)?;
        Ok(pkg_fidl::ResolvedBlob {
            data: HandleRef { raw: handle },
            content_length: blob.length,
            verified_digest: digest,
        })
    });
    let (status, blob) = match result {
        Ok(blob) => (Error::Ok, vec![blob]),
        Err(status) => (status, Vec::new()),
    };
    send(
        Channel(channel),
        &pkg_fidl::PackageResolverResolveArtifactResponse {
            status,
            blob: WireVector::from_slice(&blob),
        },
    );
}
fn credential_reply(channel: u64, result: Result<()>) {
    send(
        Channel(channel),
        &pkg_fidl::CredentialManagerRemoveRegistryCredentialResponse {
            status: result.err().unwrap_or(Error::Ok),
        },
    );
}
fn send(channel: Channel, response: &impl FidlEncode) {
    let mut bytes = vec![0; 4096];
    let mut handles = [HandleRef { raw: 0 }; 1];
    if let Ok(encoded) = response.encode(&mut bytes, &mut handles) {
        let raw: Vec<_> = handles[..encoded.handles].iter().map(|h| h.raw).collect();
        if channel.send(&bytes[..encoded.bytes], &raw).is_err() {
            for h in raw {
                let _ = Memory::close(h);
            }
        }
    } else {
        for handle in handles {
            if handle.raw != 0 {
                let _ = Memory::close(handle.raw);
            }
        }
    }
}
fn read_vmo(handle: u64, length: u64, maximum: u64) -> Result<crate::credentials::Secret> {
    if length == 0 || length > maximum {
        return Err(Error::InvalidArgs);
    }
    let (kind, rights) = Memory::object_info(handle).map_err(|_| Error::InvalidArgs)?;
    if kind != kernel_fidl::ObjectType::Vmo || rights & kernel_fidl::Rights::READ.0 == 0 {
        return Err(Error::InvalidArgs);
    }
    let address =
        Memory::map(handle, length, kernel_fidl::Rights::READ.0).map_err(|_| Error::InvalidArgs)?;
    let bytes =
        unsafe { core::slice::from_raw_parts(address as *const u8, length as usize) }.to_vec();
    let result = Memory::unmap(address, length);
    let secret = crate::credentials::Secret::new(bytes);
    result.map_err(|_| Error::Io)?;
    Ok(secret)
}
fn validate_registry(runtime: &Runtime, host: &str) -> Result<()> {
    if runtime
        .config
        .repositories
        .iter()
        .any(|repo| repo.host == host)
    {
        Ok(())
    } else {
        Err(Error::AccessDenied)
    }
}
fn sealed_write(runtime: &mut Runtime, name: &str, bytes: &[u8]) -> Result<()> {
    let mut secure_guard = runtime.secure.borrow_mut();
    let secure = secure_guard.as_mut().ok_or(Error::Unavailable)?;
    let old = secure
        .exchange(name, None)?
        .map(crate::credentials::Secret::new);
    let revision = old.as_ref().map_or(0, |record| {
        u64::from_le_bytes(record.bytes()[48..56].try_into().unwrap())
    });
    let result = secure.exchange(name, Some((revision, [0; 4], bytes)))?;
    drop(old);
    drop(result.map(crate::credentials::Secret::new));
    Ok(())
}
fn set_token(runtime: &mut Runtime, host: &str, token: &[u8], sealed: bool) -> Result<()> {
    validate_registry(runtime, host)?;
    load_sealed(runtime, host)?;
    if runtime
        .vault
        .get(host)
        .is_some_and(|c| !c.private_key.bytes().is_empty() && c.sealed != sealed)
    {
        return Err(Error::InvalidArgs);
    }
    if token.is_empty() || token.len() > 1024 || !token.iter().all(|b| (33..=126).contains(b)) {
        return Err(Error::InvalidArgs);
    }
    sealed_write(
        runtime,
        &format!("credential:{host}:token"),
        if sealed { token } else { &[] },
    )?;
    runtime.vault.set_token(host, token.to_vec(), sealed)?;
    invalidate(runtime, host);
    save_secret(runtime, host)
}
fn set_identity(
    runtime: &mut Runtime,
    host: &str,
    certificates: crate::credentials::Secret,
    private_key: crate::credentials::Secret,
    sealed: bool,
) -> Result<()> {
    validate_registry(runtime, host)?;
    load_sealed(runtime, host)?;
    if runtime
        .vault
        .get(host)
        .is_some_and(|c| !c.token.bytes().is_empty() && c.sealed != sealed)
    {
        return Err(Error::InvalidArgs);
    }
    let certificate_bytes = certificates;
    let certificates = certificate_chain(certificate_bytes.bytes())?;
    let key = rustls::pki_types::PrivateKeyDer::try_from(private_key.bytes().to_vec())
        .map_err(|_| Error::InvalidArgs)?;
    let certified =
        rustls::crypto::ring::sign::any_supported_type(&key).map_err(|_| Error::InvalidArgs)?;
    rustls::sign::CertifiedKey::new(
        certificates
            .iter()
            .cloned()
            .map(rustls::pki_types::CertificateDer::from)
            .collect(),
        certified,
    )
    .keys_match()
    .map_err(|_| Error::InvalidArgs)?;
    let generation = runtime.vault.next_generation()?;
    if sealed {
        if private_key.bytes().len() > 3040 {
            return Err(Error::ResourceExhausted);
        }
        let digest = runtime
            .secure
            .borrow_mut()
            .as_mut()
            .ok_or(Error::Unavailable)?
            .put_public(certificate_bytes.bytes())?;
        let mut payload = digest.to_vec();
        payload.extend_from_slice(private_key.bytes());
        let secret = crate::credentials::Secret::new(payload);
        sealed_write(runtime, &format!("credential:{host}:key"), secret.bytes())?;
    } else {
        sealed_write(runtime, &format!("credential:{host}:key"), &[])?;
    }
    if let Some(credential) = runtime.vault.entries.get_mut(host) {
        credential.certificates = certificates;
        credential.private_key = private_key;
        credential.sealed = sealed;
        credential.generation = generation;
    } else {
        runtime.vault.entries.insert(
            host.into(),
            crate::credentials::Credential {
                token: crate::credentials::Secret::new(Vec::new()),
                certificates,
                private_key,
                sealed,
                generation,
            },
        );
    }
    invalidate(runtime, host);
    save_secret(runtime, host)
}
pub fn save_secret(runtime: &mut Runtime, host: &str) -> Result<()> {
    use bexos_migration::codec::Encoder;
    let credential = runtime.vault.get(host).ok_or(Error::NotFound)?;
    let mut writer = Encoder::new();
    writer.bytes(if credential.sealed {
        &[]
    } else {
        credential.token.bytes()
    });
    writer.bytes(if credential.sealed {
        &[]
    } else {
        credential.private_key.bytes()
    });
    writer.word(credential.certificates.len() as u64);
    for certificate in &credential.certificates {
        writer.bytes(certificate);
    }
    let secret = crate::credentials::Secret::new(writer.finish());
    let handle = Memory::from_bytes(secret.bytes()).map_err(|_| Error::ResourceExhausted)?;
    let state = SecretHandle {
        handle,
        length: secret.bytes().len() as u64,
        sealed: credential.sealed,
    };
    if let Some(old) = runtime.secrets.insert(host.into(), state) {
        let _ = Memory::close(old.handle);
    }
    Ok(())
}
pub fn restore_secrets(runtime: &mut Runtime) -> Result<()> {
    use bexos_migration::codec::Decoder;
    for (host, handle) in &runtime.secrets {
        if handle.sealed {
            continue;
        }
        let secret = read_vmo(handle.handle, handle.length, 32 * 1024)?;
        let mut reader = Decoder::new(secret.bytes());
        let decode = |_: bexos_migration::Error| Error::VerifyFailed;
        let token = crate::credentials::Secret::new(reader.bytes(1024).map_err(decode)?.to_vec());
        let private_key =
            crate::credentials::Secret::new(reader.bytes(3072).map_err(decode)?.to_vec());
        let mut certificates = Vec::new();
        for _ in 0..reader.count(16).map_err(decode)? {
            certificates.push(reader.bytes(16 * 1024).map_err(decode)?.to_vec());
        }
        reader.finish().map_err(decode)?;
        runtime.vault.entries.insert(
            host.clone(),
            crate::credentials::Credential {
                token,
                private_key,
                certificates,
                sealed: handle.sealed,
                generation: 1,
            },
        );
    }
    let hosts: Vec<_> = runtime
        .secrets
        .iter()
        .filter(|(_, secret)| secret.sealed)
        .map(|(host, _)| host.clone())
        .collect();
    for host in hosts {
        load_sealed(runtime, &host)?;
    }
    Ok(())
}

fn invalidate(runtime: &mut Runtime, host: &str) {
    for job in &mut runtime.jobs {
        if job.query.registry_host == host {
            job.future = None;
        }
    }
}
pub fn load_sealed(runtime: &mut Runtime, host: &str) -> Result<()> {
    if runtime.vault.get(host).is_some() {
        return Ok(());
    }
    let mut secure_guard = runtime.secure.borrow_mut();
    let secure = secure_guard.as_mut().ok_or(Error::Unavailable)?;
    let token_record = secure
        .exchange(&format!("credential:{host}:token"), None)?
        .map(crate::credentials::Secret::new);
    let key_record = secure
        .exchange(&format!("credential:{host}:key"), None)?
        .map(crate::credentials::Secret::new);
    let token = token_record
        .as_ref()
        .map_or(&[][..], |record| &record.bytes()[96..]);
    let key = key_record
        .as_ref()
        .map_or(&[][..], |record| &record.bytes()[96..]);
    if token.is_empty() && key.is_empty() {
        return Ok(());
    }
    let (certificates, private_key) = if key.is_empty() {
        (Vec::new(), crate::credentials::Secret::new(Vec::new()))
    } else {
        if key.len() <= 32 {
            return Err(Error::VerifyFailed);
        }
        (
            certificate_chain(&secure.get_public(&key[..32])?)?,
            crate::credentials::Secret::new(key[32..].to_vec()),
        )
    };
    runtime.vault.entries.insert(
        host.into(),
        crate::credentials::Credential {
            token: crate::credentials::Secret::new(token.to_vec()),
            certificates,
            private_key,
            sealed: true,
            generation: 1,
        },
    );
    drop(secure_guard);
    save_secret(runtime, host)
}
fn certificate_chain(mut bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut certificates = Vec::new();
    while !bytes.is_empty() {
        if certificates.len() >= 16 || bytes.len() < 2 || bytes[0] != 0x30 {
            return Err(Error::InvalidArgs);
        }
        let (header, size) = if bytes[1] < 128 {
            (2, bytes[1] as usize)
        } else {
            let count = (bytes[1] & 127) as usize;
            if count == 0 || count > 4 || bytes.len() < 2 + count {
                return Err(Error::InvalidArgs);
            }
            let size = bytes[2..2 + count]
                .iter()
                .fold(0usize, |value, byte| (value << 8) | *byte as usize);
            (2 + count, size)
        };
        let end = header
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or(Error::InvalidArgs)?;
        certificates.push(bytes[..end].to_vec());
        bytes = &bytes[end..];
    }
    if certificates.is_empty() {
        return Err(Error::InvalidArgs);
    }
    Ok(certificates)
}
