extern crate alloc;

pub mod migration;
pub mod runtime;
pub mod wire;
mod x509;

use alloc::string::String;
use alloc::vec::Vec;
use bexos_trust_store::{
    AppSigningRootAnchor, RevocationPayload, TrustStoreError, TrustTier, decode_app_anchor,
    decode_revocation_payload, encode_app_anchor, encode_revocation_payload,
    validate_direct_app_signature,
};
use bexos_userspace::service_binding::{BoundServiceEndpoint, ServiceBinding};
use bexos_userspace::{Channel, Memory, Startup, log, yield_now};
use trust_fidl::{
    AppTrustManagerInstallEnterpriseRootResponse, AppTrustManagerUpdateRevocationListRequest,
    AppTrustManagerUpdateRevocationListResponse, AppTrustManagerValidateAppSignerRequest,
    AppTrustManagerValidateAppSignerResponse, FidlDecode, FidlEncode, HandleRef,
    TlsRootBundleFormat, TlsTrustManagerGetTlsRootBundleResponse, TrustStatus,
};
use x509::{ParsedCertificate, X509PublicKey, X509SignatureAlgorithm};

pub use runtime::main;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerificationStatus {
    Valid,
    UntrustedRoot,
    PrefixViolation,
    ExpiredCertificate,
    RevokedCertificate,
    InsufficientTier,
    UnsupportedChain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppValidationResult {
    pub status: VerificationStatus,
    pub granted_tier: TrustTier,
    pub root_anchor_id: String,
    pub leaf_certificate_fingerprint: [u8; 32],
    pub signature_algorithm: trust_fidl::SignatureAlgorithm,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrustdService {
    pub(crate) app_roots: Vec<AppSigningRootAnchor>,
    pub(crate) enterprise_roots: Vec<AppSigningRootAnchor>,
    pub(crate) revocation: Option<RevocationPayload>,
    pub(crate) tls_roots_redb: Vec<u8>,
    pub(crate) generation: u64,
    pub(crate) prod_template_disabled: bool,
}

impl TrustdService {
    pub fn new(app_roots: Vec<AppSigningRootAnchor>) -> Self {
        Self {
            app_roots,
            enterprise_roots: Vec::new(),
            revocation: None,
            tls_roots_redb: Vec::new(),
            generation: 1,
            prod_template_disabled: false,
        }
    }

    pub fn with_tls_roots(app_roots: Vec<AppSigningRootAnchor>, tls_roots_redb: Vec<u8>) -> Self {
        Self {
            app_roots,
            enterprise_roots: Vec::new(),
            revocation: None,
            tls_roots_redb,
            generation: 1,
            prod_template_disabled: false,
        }
    }

    pub fn from_root_store_bytes(
        tls_roots_redb: Vec<u8>,
        app_roots_redb: &[u8],
    ) -> Result<Self, TrustStoreError> {
        Ok(Self::with_tls_roots(
            runtime::load_app_roots(app_roots_redb)?,
            tls_roots_redb,
        ))
    }

    pub fn tls_root_bundle(&self) -> (&[u8], u64) {
        (&self.tls_roots_redb, self.generation)
    }

    pub fn app_roots(&self) -> &[AppSigningRootAnchor] {
        &self.app_roots
    }

    pub fn enterprise_roots(&self) -> &[AppSigningRootAnchor] {
        &self.enterprise_roots
    }

    pub fn all_app_roots(&self) -> Vec<AppSigningRootAnchor> {
        let mut roots = self.app_roots.clone();
        roots.extend(self.enterprise_roots.clone());
        roots
    }

    pub(crate) fn app_roots_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.app_roots.len() as u32).to_le_bytes());
        for root in &self.app_roots {
            let bytes = encode_app_anchor(root);
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&bytes);
        }
        out
    }

    pub(crate) fn replace_app_roots_from_bytes(
        &mut self,
        bytes: &[u8],
    ) -> Result<(), TrustStoreError> {
        if bytes.len() < 4 {
            return Err(TrustStoreError::CorruptRecord);
        }
        let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let mut offset = 4usize;
        let mut roots = Vec::new();
        for _ in 0..count {
            let raw_len = bytes
                .get(offset..offset + 4)
                .ok_or(TrustStoreError::CorruptRecord)?;
            let len = u32::from_le_bytes([raw_len[0], raw_len[1], raw_len[2], raw_len[3]]) as usize;
            offset = offset
                .checked_add(4)
                .ok_or(TrustStoreError::LengthOverflow)?;
            let end = offset
                .checked_add(len)
                .ok_or(TrustStoreError::LengthOverflow)?;
            roots.push(decode_app_anchor(
                bytes
                    .get(offset..end)
                    .ok_or(TrustStoreError::CorruptRecord)?,
            )?);
            offset = end;
        }
        if offset != bytes.len() {
            return Err(TrustStoreError::CorruptRecord);
        }
        self.app_roots = roots;
        Ok(())
    }

    pub(crate) fn dynamic_state_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.enterprise_roots.len() as u32).to_le_bytes());
        for root in &self.enterprise_roots {
            let bytes = encode_app_anchor(root);
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&bytes);
        }
        if let Some(revocation) = &self.revocation {
            let bytes = encode_revocation_payload(revocation);
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&bytes);
        } else {
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        out
    }

    pub(crate) fn replace_dynamic_state_from_bytes(
        &mut self,
        bytes: &[u8],
    ) -> Result<(), TrustStoreError> {
        if bytes.len() < 8 {
            return Err(TrustStoreError::CorruptRecord);
        }
        let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let mut offset = 4usize;
        let mut roots = Vec::new();
        for _ in 0..count {
            let raw_len = bytes
                .get(offset..offset + 4)
                .ok_or(TrustStoreError::CorruptRecord)?;
            let len = u32::from_le_bytes([raw_len[0], raw_len[1], raw_len[2], raw_len[3]]) as usize;
            offset = offset
                .checked_add(4)
                .ok_or(TrustStoreError::LengthOverflow)?;
            let end = offset
                .checked_add(len)
                .ok_or(TrustStoreError::LengthOverflow)?;
            roots.push(decode_app_anchor(
                bytes
                    .get(offset..end)
                    .ok_or(TrustStoreError::CorruptRecord)?,
            )?);
            offset = end;
        }
        let raw_len = bytes
            .get(offset..offset + 4)
            .ok_or(TrustStoreError::CorruptRecord)?;
        let len = u32::from_le_bytes([raw_len[0], raw_len[1], raw_len[2], raw_len[3]]) as usize;
        offset = offset
            .checked_add(4)
            .ok_or(TrustStoreError::LengthOverflow)?;
        let end = offset
            .checked_add(len)
            .ok_or(TrustStoreError::LengthOverflow)?;
        self.revocation = if len == 0 {
            None
        } else {
            Some(decode_revocation_payload(
                bytes
                    .get(offset..end)
                    .ok_or(TrustStoreError::CorruptRecord)?,
            )?)
        };
        if end != bytes.len() {
            return Err(TrustStoreError::CorruptRecord);
        }
        self.enterprise_roots = roots;
        Ok(())
    }

    pub fn validate_direct_ed25519(
        &self,
        package_id: &str,
        now: u64,
        public_key: &[u8],
    ) -> AppValidationResult {
        match validate_direct_app_signature(&self.all_app_roots(), package_id, now, public_key) {
            Ok(anchor) => AppValidationResult {
                status: VerificationStatus::Valid,
                granted_tier: anchor.tier,
                root_anchor_id: anchor.anchor_id,
                leaf_certificate_fingerprint: blake3::hash(public_key).into(),
                signature_algorithm: trust_fidl::SignatureAlgorithm::Ed25519,
            },
            Err(TrustStoreError::InvalidPrefix) => AppValidationResult {
                status: VerificationStatus::PrefixViolation,
                granted_tier: TrustTier::Tier4WebOrigin,
                root_anchor_id: String::new(),
                leaf_certificate_fingerprint: [0; 32],
                signature_algorithm: trust_fidl::SignatureAlgorithm::Ed25519,
            },
            Err(TrustStoreError::InvalidValidity) => AppValidationResult {
                status: VerificationStatus::ExpiredCertificate,
                granted_tier: TrustTier::Tier4WebOrigin,
                root_anchor_id: String::new(),
                leaf_certificate_fingerprint: [0; 32],
                signature_algorithm: trust_fidl::SignatureAlgorithm::Ed25519,
            },
            _ => AppValidationResult {
                status: VerificationStatus::UntrustedRoot,
                granted_tier: TrustTier::Tier4WebOrigin,
                root_anchor_id: String::new(),
                leaf_certificate_fingerprint: [0; 32],
                signature_algorithm: trust_fidl::SignatureAlgorithm::Ed25519,
            },
        }
    }

    pub fn validate_app_signer(
        &self,
        package_id: &str,
        now: u64,
        algorithm: trust_fidl::SignatureAlgorithm,
        signer_chain: &[Vec<u8>],
        payload_digest: &[u8; 32],
        signature: &[u8],
        required_tier: TrustTier,
    ) -> AppValidationResult {
        if self.prod_template_disabled {
            return invalid(VerificationStatus::UntrustedRoot, algorithm);
        }
        if signer_chain.is_empty() || signer_chain.len() > 4 {
            return invalid(VerificationStatus::UnsupportedChain, algorithm);
        }
        let leaf = &signer_chain[0];
        if self.is_revoked(leaf) {
            return invalid(VerificationStatus::RevokedCertificate, algorithm);
        }
        if algorithm == trust_fidl::SignatureAlgorithm::Ed25519 && leaf.len() == 32 {
            if !verify_ed25519(leaf, payload_digest, signature) {
                return invalid(VerificationStatus::UnsupportedChain, algorithm);
            }
            let result = self.validate_direct_ed25519(package_id, now, leaf);
            return require_tier(result, required_tier);
        }

        match self.validate_x509_chain(
            package_id,
            now,
            algorithm,
            signer_chain,
            payload_digest,
            signature,
            required_tier,
        ) {
            Some(result) => result,
            None => invalid(VerificationStatus::UntrustedRoot, algorithm),
        }
    }

    pub fn update_revocation(
        &mut self,
        payload: RevocationPayload,
        now: u64,
    ) -> Result<(), TrustStoreError> {
        if now < payload.valid_from || now > payload.valid_until {
            return Err(TrustStoreError::InvalidValidity);
        }
        if self
            .revocation
            .as_ref()
            .is_some_and(|current| payload.generation <= current.generation)
        {
            return Err(TrustStoreError::InvalidValidity);
        }
        for root in &self.app_roots {
            let cert = if root.certificate_der.is_empty() {
                &root.public_key_bytes
            } else {
                &root.certificate_der
            };
            let fingerprint: [u8; 32] = blake3::hash(cert).into();
            if root.immutable && payload.revoked_cert_fingerprints.contains(&fingerprint) {
                return Err(TrustStoreError::InvalidPrefix);
            }
        }
        self.revocation = Some(payload);
        self.generation = self.generation.saturating_add(1);
        Ok(())
    }

    pub fn install_enterprise_root(
        &mut self,
        root_cert: Vec<u8>,
        prefixes: Vec<String>,
    ) -> Result<String, TrustStoreError> {
        if root_cert.is_empty() || prefixes.is_empty() {
            return Err(TrustStoreError::EmptyBytes);
        }
        if prefixes.iter().any(|prefix| {
            prefix.starts_with("bexos.") || prefix.starts_with("com.bexos.") || prefix.is_empty()
        }) {
            return Err(TrustStoreError::InvalidPrefix);
        }
        let id = format!("enterprise-{}", hex32(&blake3::hash(&root_cert).into()));
        let anchor = AppSigningRootAnchor {
            anchor_id: id.clone(),
            tier: TrustTier::Tier3Enterprise,
            algorithm: "Ed25519".into(),
            public_key_bytes: Vec::new(),
            certificate_der: root_cert,
            permitted_package_prefixes: prefixes,
            valid_from: 0,
            valid_until: 0,
            is_hardware_anchored: false,
            immutable: false,
            enterprise: true,
        };
        bexos_trust_store::validate_app_anchor(&anchor)?;
        self.enterprise_roots.retain(|root| root.anchor_id != id);
        self.enterprise_roots.push(anchor);
        self.generation = self.generation.saturating_add(1);
        Ok(id)
    }

    pub fn remove_enterprise_root(&mut self, anchor_id: &str) -> Result<(), TrustStoreError> {
        if self
            .app_roots
            .iter()
            .any(|root| root.anchor_id == anchor_id && root.immutable)
        {
            return Err(TrustStoreError::InvalidPrefix);
        }
        let before = self.enterprise_roots.len();
        self.enterprise_roots
            .retain(|root| root.anchor_id != anchor_id || !root.enterprise);
        if self.enterprise_roots.len() == before {
            return Err(TrustStoreError::NotFound);
        }
        self.generation = self.generation.saturating_add(1);
        Ok(())
    }

    fn is_revoked(&self, cert: &[u8]) -> bool {
        let Some(revocation) = &self.revocation else {
            return false;
        };
        let fingerprint: [u8; 32] = blake3::hash(cert).into();
        revocation.revoked_cert_fingerprints.contains(&fingerprint)
    }

    fn is_cert_revoked(&self, cert: &ParsedCertificate<'_>) -> bool {
        let Some(revocation) = &self.revocation else {
            return false;
        };
        let cert_fingerprint: [u8; 32] = blake3::hash(cert.raw_der).into();
        let spki_fingerprint: [u8; 32] = blake3::hash(cert.spki_der).into();
        revocation
            .revoked_cert_fingerprints
            .contains(&cert_fingerprint)
            || revocation
                .revoked_spki_fingerprints
                .contains(&spki_fingerprint)
            || revocation.revoked_serials.contains(&cert.serial)
    }

    fn validate_x509_chain(
        &self,
        package_id: &str,
        now: u64,
        algorithm: trust_fidl::SignatureAlgorithm,
        signer_chain: &[Vec<u8>],
        payload_digest: &[u8; 32],
        signature: &[u8],
        required_tier: TrustTier,
    ) -> Option<AppValidationResult> {
        let parsed = signer_chain
            .iter()
            .map(|cert| x509::parse_certificate(cert))
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        if parsed.is_empty() {
            return Some(invalid(VerificationStatus::UnsupportedChain, algorithm));
        }
        for cert in &parsed {
            if now < cert.not_before || now > cert.not_after {
                return Some(invalid(VerificationStatus::ExpiredCertificate, algorithm));
            }
            if self.is_cert_revoked(cert) {
                return Some(invalid(VerificationStatus::RevokedCertificate, algorithm));
            }
        }
        let leaf = &parsed[0];
        if leaf.is_ca || !leaf.has_key_usage || !leaf.digital_signature {
            return Some(invalid(VerificationStatus::UnsupportedChain, algorithm));
        }
        if !leaf.has_eku || !leaf.code_signing_eku {
            return Some(invalid(VerificationStatus::UnsupportedChain, algorithm));
        }
        for window in parsed.windows(2) {
            let cert = &window[0];
            let issuer = &window[1];
            if cert.issuer_der != issuer.subject_der
                || !issuer.is_ca
                || !issuer.has_key_usage
                || !issuer.key_cert_sign
                || !x509::verify_certificate_signature(cert, issuer)
            {
                return Some(invalid(VerificationStatus::UnsupportedChain, algorithm));
            }
        }
        let selected_root = parsed.last()?;
        if selected_root.issuer_der != selected_root.subject_der
            || !selected_root.is_ca
            || !selected_root.has_key_usage
            || !selected_root.key_cert_sign
            || !x509::verify_certificate_signature(selected_root, selected_root)
        {
            return Some(invalid(VerificationStatus::UnsupportedChain, algorithm));
        }
        if !verify_package_signature(leaf.public_key, algorithm, payload_digest, signature) {
            return Some(invalid(VerificationStatus::UnsupportedChain, algorithm));
        }
        for root in self.all_app_roots() {
            if root.certificate_der.is_empty() || root.certificate_der != selected_root.raw_der {
                continue;
            }
            if !bexos_trust_store::package_permitted(&root, package_id) {
                return Some(invalid(VerificationStatus::PrefixViolation, algorithm));
            }
            if now < root.valid_from || (root.valid_until != 0 && now > root.valid_until) {
                return Some(invalid(VerificationStatus::ExpiredCertificate, algorithm));
            }
            if self.is_cert_revoked(selected_root) {
                return Some(invalid(VerificationStatus::RevokedCertificate, algorithm));
            }
            if root.tier as u8 > required_tier as u8 {
                return Some(invalid(VerificationStatus::InsufficientTier, algorithm));
            }
            return Some(AppValidationResult {
                status: VerificationStatus::Valid,
                granted_tier: root.tier,
                root_anchor_id: root.anchor_id,
                leaf_certificate_fingerprint: blake3::hash(leaf.raw_der).into(),
                signature_algorithm: algorithm,
            });
        }
        None
    }
}

fn verify_ed25519(public_key: &[u8], payload_digest: &[u8; 32], signature: &[u8]) -> bool {
    verify_ed25519_message(public_key, payload_digest, signature)
}

fn verify_package_signature(
    public_key: X509PublicKey<'_>,
    algorithm: trust_fidl::SignatureAlgorithm,
    payload_digest: &[u8; 32],
    signature: &[u8],
) -> bool {
    match algorithm {
        trust_fidl::SignatureAlgorithm::Ed25519 => x509::verify_signature(
            public_key,
            X509SignatureAlgorithm::Ed25519,
            payload_digest,
            signature,
        ),
        trust_fidl::SignatureAlgorithm::EcdsaP256Sha256 => x509::verify_signature(
            public_key,
            X509SignatureAlgorithm::EcdsaP256Sha256,
            payload_digest,
            signature,
        ),
    }
}

fn verify_ed25519_message(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let Ok(public_key) = <&[u8; 32]>::try_from(public_key) else {
        return false;
    };
    let Ok(signature) = <&[u8; 64]>::try_from(signature) else {
        return false;
    };
    let Ok(verifying) = ed25519_dalek::VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    let signature = ed25519_dalek::Signature::from_bytes(signature);
    ed25519_dalek::Verifier::verify(&verifying, message, &signature).is_ok()
}

fn invalid(
    status: VerificationStatus,
    algorithm: trust_fidl::SignatureAlgorithm,
) -> AppValidationResult {
    AppValidationResult {
        status,
        granted_tier: TrustTier::Tier4WebOrigin,
        root_anchor_id: String::new(),
        leaf_certificate_fingerprint: [0; 32],
        signature_algorithm: algorithm,
    }
}

fn require_tier(mut result: AppValidationResult, required_tier: TrustTier) -> AppValidationResult {
    if result.status == VerificationStatus::Valid && result.granted_tier as u8 > required_tier as u8
    {
        result.status = VerificationStatus::InsufficientTier;
    }
    result
}

fn hex32(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::new();
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}
pub fn run(channel: u64) -> ! {
    let manager = Channel(channel);
    let startup = Startup::receive(manager).expect("trustd startup");
    if startup.migration_target {
        bexos_userspace::exit();
    }
    if startup.resources.len() != 2 || startup.arg0 == 0 || startup.arg1 == 0 {
        log("trustd: missing root store startup resources\n");
        bexos_userspace::exit();
    }
    let tls_len = bexos_boot::page_round(startup.arg0).expect("trustd tls store size");
    let app_len = bexos_boot::page_round(startup.arg1).expect("trustd app store size");
    let tls_va = Memory::map(startup.resources[0], tls_len, 2).expect("trustd tls roots map");
    let tls_roots_redb =
        unsafe { core::slice::from_raw_parts(tls_va as *const u8, startup.arg0 as usize) }.to_vec();
    let app_va = Memory::map(startup.resources[1], app_len, 2).expect("trustd app roots map");
    Memory::unmap(tls_va, tls_len).expect("trustd tls roots unmap");
    Memory::unmap(app_va, app_len).expect("trustd app roots unmap");
    startup.close_resources();
    let service = TrustdService::with_tls_roots(Vec::new(), tls_roots_redb);
    let _ = Startup::ready(manager);
    log("trustd: service ready; BootFS root stores available for early validation\n");
    serve(manager, service)
}

fn serve(manager: Channel, service: TrustdService) -> ! {
    let mut app_clients = Vec::new();
    let mut tls_clients = Vec::new();
    loop {
        if let Ok(message) = manager.try_recv() {
            if let (Some(endpoint), Ok(metadata)) = (
                message.handles.first().copied(),
                core::str::from_utf8(&message.bytes),
            ) {
                match metadata_protocol(metadata) {
                    Some("AppTrustManager") | Some("TlsTrustManager") => {}
                    _ => {}
                }
                if let Some(binding) = ServiceBinding::parse(metadata) {
                    if binding.protocol_is("AppTrustManager") {
                        app_clients.push(BoundServiceEndpoint::new(
                            Channel(endpoint),
                            binding.method_ordinals,
                        ));
                    } else if binding.protocol_is("TlsTrustManager") {
                        tls_clients.push(BoundServiceEndpoint::new(
                            Channel(endpoint),
                            binding.method_ordinals,
                        ));
                    }
                }
            }
        }
        poll_app_clients(&mut app_clients, &service);
        poll_tls_clients(&mut tls_clients, &service);
        yield_now();
    }
}

fn poll_app_clients(clients: &mut Vec<BoundServiceEndpoint>, service: &TrustdService) {
    clients.retain(|client| match client.channel.try_recv() {
        Ok(message) => {
            let (ordinal, req) = envelope(&message.bytes);
            let handles = handle_refs(&message.handles);
            if !client.allows(ordinal) {
                return true;
            }
            match ordinal {
                1 => reply_validate_app(client.channel, service, req, &handles),
                2 => reply(
                    client.channel,
                    &AppTrustManagerInstallEnterpriseRootResponse {
                        status: TrustStatus::ErrUnsupported,
                    },
                ),
                3 => {
                    let _ = AppTrustManagerUpdateRevocationListRequest::decode(req, &handles);
                    reply(
                        client.channel,
                        &AppTrustManagerUpdateRevocationListResponse {
                            status: TrustStatus::ErrUnsupported,
                        },
                    )
                }
                _ => {}
            }
            true
        }
        Err(kernel_fidl::Status::ErrPeerClosed) => false,
        Err(_) => true,
    });
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

fn poll_tls_clients(clients: &mut Vec<BoundServiceEndpoint>, service: &TrustdService) {
    clients.retain(|client| match client.channel.try_recv() {
        Ok(message) => {
            let (ordinal, req) = envelope(&message.bytes);
            if client.allows(ordinal) && ordinal == 1 {
                let _ = req;
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
        Err(kernel_fidl::Status::ErrPeerClosed) => false,
        Err(_) => true,
    });
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

fn metadata_protocol(metadata: &str) -> Option<&str> {
    metadata.split('|').nth(1)
}
