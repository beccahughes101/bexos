use alloc::string::String;
use alloc::vec::Vec;

use bexos_trust_store::TrustTier;
use bexos_userspace::service_binding::BoundServiceEndpoint;
use bexos_userspace::{Channel, Memory};
use trust_fidl::{
    AppTrustManagerInstallEnterpriseRootResponse, AppTrustManagerListEnterpriseRootsResponse,
    AppTrustManagerRemoveEnterpriseRootRequest, AppTrustManagerRemoveEnterpriseRootResponse,
    AppTrustManagerUpdateRevocationListRequest, AppTrustManagerUpdateRevocationListResponse,
    AppTrustManagerValidateAppSignerRequest, AppTrustManagerValidateAppSignerResponse, FidlDecode,
    FidlEncode, HandleRef, TlsRootBundleFormat, TlsTrustManagerGetTlsRootBundleResponse,
    TrustStatus, WireStringVector,
};

use crate::{AppValidationResult, TrustdService, VerificationStatus};

pub fn poll_app_clients(
    clients: &mut Vec<BoundServiceEndpoint>,
    service: &mut TrustdService,
) -> bool {
    let mut changed = false;
    clients.retain(|client| match client.channel.try_recv() {
        Ok(message) => {
            let (ordinal, req) = envelope(&message.bytes);
            let handles = handle_refs(&message.handles);
            if !client.allows(ordinal) {
                return true;
            }
            match ordinal {
                1 => reply_validate_app(client.channel, service, req, &handles),
                2 => reply_install_enterprise_root(client.channel, service, req, &handles),
                3 => reply_update_revocation(client.channel, service, req, &handles),
                4 => reply_remove_enterprise_root(client.channel, service, req, &handles),
                5 => reply_list_enterprise_roots(client.channel, service),
                _ => {}
            }
            true
        }
        Err(kernel_fidl::Status::ErrPeerClosed) => {
            changed = true;
            false
        }
        Err(_) => true,
    });
    changed
}

pub fn poll_tls_clients(clients: &mut Vec<BoundServiceEndpoint>, service: &TrustdService) -> bool {
    let mut changed = false;
    clients.retain(|client| match client.channel.try_recv() {
        Ok(message) => {
            let (ordinal, _) = envelope(&message.bytes);
            if client.allows(ordinal) && ordinal == 1 {
                let (bytes, generation) = service.tls_root_bundle();
                let (status, bundle) = if bytes.is_empty() {
                    (TrustStatus::ErrStorage, 0)
                } else {
                    match Memory::from_bytes(bytes) {
                        Ok(vmo) => (TrustStatus::Ok, vmo),
                        Err(_) => (TrustStatus::ErrStorage, 0),
                    }
                };
                reply(
                    client.channel,
                    &TlsTrustManagerGetTlsRootBundleResponse {
                        status,
                        bundle: HandleRef { raw: bundle },
                        length: bytes.len() as u64,
                        generation,
                        format: TlsRootBundleFormat::RedbTlsRoots,
                    },
                );
            }
            true
        }
        Err(kernel_fidl::Status::ErrPeerClosed) => {
            changed = true;
            false
        }
        Err(_) => true,
    });
    changed
}

pub fn metadata_protocol(metadata: &str) -> Option<&str> {
    metadata.split('|').nth(1)
}

fn reply_validate_app(
    channel: Channel,
    service: &TrustdService,
    req: &[u8],
    handles: &[HandleRef],
) {
    let Ok(request) = AppTrustManagerValidateAppSignerRequest::decode(req, handles) else {
        let result = AppValidationResult {
            status: VerificationStatus::UnsupportedChain,
            granted_tier: TrustTier::Tier4WebOrigin,
            root_anchor_id: String::new(),
            leaf_certificate_fingerprint: [0; 32],
            signature_algorithm: trust_fidl::SignatureAlgorithm::Ed25519,
        };
        reply(
            channel,
            &AppTrustManagerValidateAppSignerResponse {
                status: TrustStatus::ErrInvalidArgs,
                result: wire_app_result(&result),
            },
        );
        return;
    };
    let chain = request
        .signer_cert_chain
        .iter()
        .map(|cert| cert.to_vec())
        .collect::<Vec<_>>();
    let required_tier =
        TrustTier::from_u64(u64::from(request.required_tier)).unwrap_or(TrustTier::Tier4WebOrigin);
    let result = service.validate_app_signer(
        request.package_id,
        0,
        request.signature_algorithm,
        &chain,
        &request.payload_digest,
        request.signature,
        required_tier,
    );
    reply(
        channel,
        &AppTrustManagerValidateAppSignerResponse {
            status: TrustStatus::Ok,
            result: wire_app_result(&result),
        },
    );
}

fn wire_app_result(result: &AppValidationResult) -> trust_fidl::AppValidationResult<'_> {
    trust_fidl::AppValidationResult {
        status: match result.status {
            VerificationStatus::Valid => trust_fidl::VerificationStatus::Valid,
            VerificationStatus::UntrustedRoot => trust_fidl::VerificationStatus::UntrustedRoot,
            VerificationStatus::PrefixViolation => trust_fidl::VerificationStatus::PrefixViolation,
            VerificationStatus::ExpiredCertificate => {
                trust_fidl::VerificationStatus::ExpiredCertificate
            }
            VerificationStatus::RevokedCertificate => {
                trust_fidl::VerificationStatus::RevokedCertificate
            }
            VerificationStatus::InsufficientTier => {
                trust_fidl::VerificationStatus::InsufficientTier
            }
            VerificationStatus::UnsupportedChain => {
                trust_fidl::VerificationStatus::UnsupportedChain
            }
        },
        granted_tier: result.granted_tier as u8,
        root_anchor_id: &result.root_anchor_id,
        leaf_certificate_fingerprint: result.leaf_certificate_fingerprint,
        signature_algorithm: result.signature_algorithm,
    }
}

fn reply_install_enterprise_root(
    channel: Channel,
    service: &mut TrustdService,
    req: &[u8],
    handles: &[HandleRef],
) {
    let status = match trust_fidl::AppTrustManagerInstallEnterpriseRootRequest::decode(req, handles)
    {
        Ok(request) => service
            .install_enterprise_root(
                request.root_cert.to_vec(),
                collect_strings(request.permitted_prefixes).unwrap_or_default(),
            )
            .map(|_| TrustStatus::Ok)
            .unwrap_or(TrustStatus::ErrInvalidArgs),
        Err(_) => TrustStatus::ErrInvalidArgs,
    };
    reply(
        channel,
        &AppTrustManagerInstallEnterpriseRootResponse { status },
    );
}

fn reply_update_revocation(
    channel: Channel,
    service: &mut TrustdService,
    req: &[u8],
    handles: &[HandleRef],
) {
    let status = match AppTrustManagerUpdateRevocationListRequest::decode(req, handles) {
        Ok(request) => {
            let bytes = read_vmo(request.revocation_payload.raw, request.length);
            bexos_trust_store::decode_revocation_payload(&bytes)
                .and_then(|payload| service.update_revocation(payload, 0))
                .map(|_| TrustStatus::Ok)
                .unwrap_or(TrustStatus::ErrInvalidArgs)
        }
        Err(_) => TrustStatus::ErrInvalidArgs,
    };
    reply(
        channel,
        &AppTrustManagerUpdateRevocationListResponse { status },
    );
}

fn reply_remove_enterprise_root(
    channel: Channel,
    service: &mut TrustdService,
    req: &[u8],
    handles: &[HandleRef],
) {
    let status = match AppTrustManagerRemoveEnterpriseRootRequest::decode(req, handles) {
        Ok(request) => service
            .remove_enterprise_root(request.anchor_id)
            .map(|_| TrustStatus::Ok)
            .unwrap_or(TrustStatus::ErrInvalidArgs),
        Err(_) => TrustStatus::ErrInvalidArgs,
    };
    reply(
        channel,
        &AppTrustManagerRemoveEnterpriseRootResponse { status },
    );
}

fn reply_list_enterprise_roots(channel: Channel, service: &TrustdService) {
    let roots = service
        .enterprise_roots
        .iter()
        .map(|root| root.anchor_id.as_str())
        .collect::<Vec<_>>();
    reply(
        channel,
        &AppTrustManagerListEnterpriseRootsResponse {
            status: TrustStatus::Ok,
            roots: WireStringVector::from_slice(&roots),
        },
    );
}

fn collect_strings(values: WireStringVector<'_>) -> Result<Vec<String>, trust_fidl::FidlWireError> {
    let mut out = Vec::new();
    for index in 0..values.len() {
        out.push(values.get(index)?.to_string());
    }
    Ok(out)
}

fn read_vmo(handle: u64, len: u64) -> Vec<u8> {
    if handle == 0 || len == 0 || len > 1024 * 1024 {
        return Vec::new();
    }
    let Some(mapped_len) = bexos_boot::page_round(len) else {
        return Vec::new();
    };
    let Ok(va) = Memory::map(handle, mapped_len, 2) else {
        return Vec::new();
    };
    let bytes = unsafe { core::slice::from_raw_parts(va as *const u8, len as usize) }.to_vec();
    let _ = Memory::unmap(va, mapped_len);
    let _ = Memory::close(handle);
    bytes
}

fn reply<T: FidlEncode>(channel: Channel, response: &T) {
    let mut out = [0; 8192];
    let mut handles = [HandleRef { raw: 0 }; 8];
    if let Ok(encoded) = response.encode(&mut out, &mut handles) {
        let raw_handles = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect::<Vec<_>>();
        let _ = channel.send(&out[..encoded.bytes], &raw_handles);
    }
}

fn handle_refs(handles: &[u64]) -> Vec<HandleRef> {
    handles.iter().map(|raw| HandleRef { raw: *raw }).collect()
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[..8]);
    (u64::from_le_bytes(raw), &bytes[8..])
}
