//! Acceptance clients may write storage only after the verified boot owner
//! has handed the RPMB connection to BexOS and initialized KeyMint's root.
use kmr_wire::{keymint::ErrorCode, *};
use tipc::Handle;

pub fn integrated() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        let monitor = unsafe { core::arch::x86_64::__cpuid(0x40000100) };
        // Discovery ABI 1, x86_64, boot-services capability in EDX. A standalone
        // monitor probe has the same transport but no BexOS KeyMint initializer.
        monitor.eax == 0x4245584d && monitor.ebx == 1 && monitor.ecx == 2 && monitor.edx & 1 != 0
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        true
    }
}

pub fn wait() -> Result<(), String> {
    if !integrated() {
        return Ok(());
    }
    let handle = Handle::connect(c"com.android.trusty.keymint")
        .map_err(|e| format!("KeyMint boot wait connect: {e:?}"))?;
    let request = PerformOpReq::GetRootOfTrust(GetRootOfTrustRequest { challenge: [0; 16] })
        .into_vec()
        .map_err(|e| format!("boot readiness encode: {e:?}"))?;
    for _ in 0..600 {
        handle
            .send(&request.as_slice())
            .map_err(|e| format!("boot readiness send: {e:?}"))?;
        let mut reply = Vec::new();
        loop {
            let mut bytes = [0; 8192];
            let mut handles = std::array::from_fn::<_, 8, _>(|_| None);
            let (size, count) = handle
                .recv_vectored(&mut [&mut bytes], &mut handles)
                .map_err(|e| format!("boot readiness receive: {e:?}"))?;
            if count != 0 || size == 0 || bytes[0] > 1 || reply.len() + size - 1 > 65536 {
                return Err("invalid boot readiness chunks".into());
            }
            reply.extend_from_slice(&bytes[1..size]);
            if bytes[0] == 0 {
                break;
            }
        }
        let response = PerformOpResponse::from_slice(&reply)
            .map_err(|e| format!("boot readiness decode: {e:?}"))?;
        if response.error_code == 0 {
            if !matches!(response.rsp, Some(PerformOpRsp::GetRootOfTrust(_))) {
                return Err("wrong boot readiness response".into());
            }
            log::info!("bexos-acceptance: authenticated boot services ready");
            return Ok(());
        }
        if response.error_code != ErrorCode::HardwareNotYetAvailable as i32 {
            return Err(format!(
                "KeyMint boot readiness status {}",
                response.error_code
            ));
        }
        if unsafe { trusty_sys::nanosleep(0, 0, 1_000_000_000) } != 0 {
            return Err("boot readiness sleep failed".into());
        }
    }
    Err("authenticated boot services readiness timed out".into())
}
