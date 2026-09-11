use kmr_wire::{keymint::*, *};
use tipc::Handle;

fn exchange(handle: &Handle, request: PerformOpReq) -> Result<PerformOpResponse, String> {
    let request = request
        .into_vec()
        .map_err(|e| format!("KeyMint encode: {e:?}"))?;
    handle
        .send(&request.as_slice())
        .map_err(|e| format!("KeyMint send: {e:?}"))?;
    let mut bytes = Vec::new();
    loop {
        let response = super::receive(handle)?;
        let (&more, content) = response
            .split_first()
            .ok_or("missing KeyMint chunk header")?;
        if more > 1 || bytes.len() + content.len() > 65536 {
            return Err("invalid KeyMint chunks".into());
        }
        bytes.extend_from_slice(content);
        if more == 0 {
            break;
        }
    }
    PerformOpResponse::from_slice(&bytes).map_err(|e| format!("KeyMint decode: {e:?}"))
}
fn invoke(handle: &Handle, request: PerformOpReq) -> Result<PerformOpRsp, String> {
    let response = exchange(handle, request)?;
    if response.error_code != 0 {
        return Err(format!("KeyMint status {}", response.error_code));
    }
    response.rsp.ok_or("missing KeyMint reply".into())
}
fn begin(blob: &[u8]) -> PerformOpReq {
    PerformOpReq::DeviceBegin(BeginRequest {
        purpose: KeyPurpose::Sign,
        key_blob: blob.to_vec(),
        params: vec![KeyParam::Digest(Digest::Sha256), KeyParam::MacLength(256)],
        auth_token: None,
    })
}
pub fn run(integrated: bool) -> Result<(), String> {
    let handle = Handle::connect(c"com.android.trusty.keymint")
        .map_err(|e| format!("KeyMint connect: {e:?}"))?;
    if !integrated {
        invoke(
            &handle,
            PerformOpReq::SetHalInfo(SetHalInfoRequest {
                os_version: 1,
                os_patchlevel: 202609,
                vendor_patchlevel: 20260905,
            }),
        )?;
        invoke(
            &handle,
            PerformOpReq::SetBootInfo(SetBootInfoRequest {
                verified_boot_key: vec![0; 32],
                device_boot_locked: false,
                verified_boot_state: 2,
                verified_boot_hash: vec![0; 32],
                boot_patchlevel: 20260905,
            }),
        )?;
    }
    let generated = invoke(
        &handle,
        PerformOpReq::DeviceGenerateKey(GenerateKeyRequest {
            key_params: vec![
                KeyParam::Purpose(KeyPurpose::Sign),
                KeyParam::Algorithm(Algorithm::Hmac),
                KeyParam::KeySize(KeySizeInBits(256)),
                KeyParam::Digest(Digest::Sha256),
                KeyParam::MinMacLength(256),
                KeyParam::NoAuthRequired,
                KeyParam::RollbackResistance,
            ],
            attestation_key: None,
        }),
    )?;
    let PerformOpRsp::DeviceGenerateKey(generated) = generated else {
        return Err("wrong generate response".into());
    };
    let blob = generated.ret.key_blob;
    if blob.is_empty() {
        return Err("empty KeyMint blob".into());
    }
    let mut previous = None;
    for _ in 0..2 {
        let PerformOpRsp::DeviceBegin(operation) = invoke(&handle, begin(&blob))? else {
            return Err("wrong begin response".into());
        };
        let output = invoke(
            &handle,
            PerformOpReq::OperationFinish(FinishRequest {
                op_handle: operation.ret.op_handle,
                input: Some(b"BexOS standalone x86 acceptance".to_vec()),
                signature: None,
                auth_token: None,
                timestamp_token: None,
                confirmation_token: None,
            }),
        )?;
        let PerformOpRsp::OperationFinish(output) = output else {
            return Err("wrong finish response".into());
        };
        if output.ret.len() != 32 {
            return Err("invalid HMAC length".into());
        }
        if previous.as_ref().is_some_and(|value| value != &output.ret) {
            return Err("HMAC changed for identical input".into());
        }
        previous = Some(output.ret);
    }
    invoke(
        &handle,
        PerformOpReq::DeviceDeleteKey(DeleteKeyRequest {
            key_blob: blob.clone(),
        }),
    )?;
    let response = exchange(&handle, begin(&blob))?;
    if response.error_code != ErrorCode::InvalidKeyBlob as i32 {
        return Err(format!("deleted key not rejected: {}", response.error_code));
    }
    log::info!("bexos-security-acceptance: KeyMint HMAC and deletion verified");
    Ok(())
}
