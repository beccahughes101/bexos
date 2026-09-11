use super::*;
use bexos_wasm_abi::signature::SignatureEnvelope;
use sha2::{Digest, Sha256};
use trust_fidl::*;
/// Signature envelope: package_id=1, repeated certificates=2, signature=3,
/// algorithm=4. The digest is always computed from copied module bytes here.
pub fn verify(trust: Option<u64>, module: &[u8], envelope: &[u8]) -> Result<()> {
    let channel = trust.ok_or_else(|| wasmtime::format_err!("AppTrustManager grant missing"))?;
    verify_with(module, envelope, |request| {
        let mut client = AppTrustManagerPublicClient::new(bexos_userspace::Rpc(Channel(channel)));
        let mut rb = vec![0; 16384];
        let mut rh = [HandleRef { raw: 0 }; 1];
        let mut ob = [0; 2048];
        let mut oh = [HandleRef { raw: 0 }; 1];
        let response = client
            .validate_app_signer(&request, &mut rb, &mut rh, &mut ob, &mut oh)
            .map_err(|_| wasmtime::format_err!("trust transport"))?;
        if response.status != TrustStatus::Ok || response.result.status != VerificationStatus::Valid
        {
            bail!("child signature not trusted");
        }
        Ok(())
    })
}
fn verify_with(
    module: &[u8],
    envelope: &[u8],
    validate: impl FnOnce(AppTrustManagerValidateAppSignerRequest<'_>) -> Result<()>,
) -> Result<()> {
    let envelope = SignatureEnvelope::decode(envelope)
        .map_err(|error| wasmtime::format_err!("invalid signature envelope: {error:?}"))?;
    let SignatureEnvelope {
        package_id: package,
        certificates: chain,
        signature,
        algorithm,
    } = envelope;
    let signature_algorithm = match algorithm {
        1 => SignatureAlgorithm::Ed25519,
        2 => SignatureAlgorithm::EcdsaP256Sha256,
        _ => bail!("unsupported signature algorithm"),
    };
    let payload_digest = Sha256::digest(module).into();
    let chain_refs: Vec<_> = chain.iter().map(|c| c.as_slice()).collect();
    let request = AppTrustManagerValidateAppSignerRequest {
        package_id: &package,
        signer_cert_chain: &chain_refs,
        payload_digest,
        signature: &signature,
        signature_algorithm,
        required_tier: 4,
    };
    validate(request)
}
#[cfg(test)]
mod tests {
    use super::*;
    use bexos_trust_store::{AppSigningRootAnchor, TrustTier};
    use bexos_trustd::TrustdService;
    use ed25519_dalek::{Signer, SigningKey};
    #[test]
    fn exact_child_bytes_and_package_binding_are_verified_by_existing_trust_service() {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let public = signing.verifying_key().to_bytes();
        let service = TrustdService::new(vec![AppSigningRootAnchor {
            anchor_id: "wasm-test-root".into(),
            tier: TrustTier::Tier1PlatformApp,
            algorithm: "Ed25519".into(),
            public_key_bytes: public.to_vec(),
            certificate_der: Vec::new(),
            permitted_package_prefixes: vec!["test.*".into()],
            valid_from: 0,
            valid_until: 0,
            is_hardware_anchored: false,
            immutable: false,
            enterprise: false,
        }]);
        let payload = b"\0asm\x01\0\0\0";
        let signature = signing.sign(&Sha256::digest(payload)).to_bytes();
        let envelope = SignatureEnvelope {
            package_id: "test.child".into(),
            certificates: vec![public.to_vec()],
            signature: signature.to_vec(),
            algorithm: 1,
        };
        let verify = |bytes: &[u8], envelope: &SignatureEnvelope| {
            verify_with(bytes, &envelope.encode().unwrap(), |request| {
                assert_eq!(request.required_tier, 4);
                let result = service.validate_app_signer(
                    request.package_id,
                    0,
                    request.signature_algorithm,
                    &request
                        .signer_cert_chain
                        .iter()
                        .map(|c| c.to_vec())
                        .collect::<Vec<_>>(),
                    &request.payload_digest,
                    request.signature,
                    TrustTier::Tier4WebOrigin,
                );
                if result.status != bexos_trustd::VerificationStatus::Valid {
                    bail!("untrusted child");
                }
                Ok(())
            })
        };
        verify(payload, &envelope).unwrap();
        let mut changed = payload.to_vec();
        changed.push(0);
        assert!(verify(&changed, &envelope).is_err());
        let mut changed = envelope.clone();
        changed.signature[0] ^= 1;
        assert!(verify(payload, &changed).is_err());
        let mut changed = envelope.clone();
        changed.package_id = "another.owner".into();
        assert!(verify(payload, &changed).is_err());
        let mut changed = envelope;
        changed.certificates[0][0] ^= 1;
        assert!(verify(payload, &changed).is_err());
    }
}
