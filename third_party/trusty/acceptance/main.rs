//! Isolated secure-world acceptance client, never included in standard firmware.
mod boot_ready;
use android_hardware_security_see_authmgr::aidl::android::hardware::security::see::authmgr::{
    DicePolicy::DicePolicy, Error::Error, ExplicitKeyDiceCertChain::ExplicitKeyDiceCertChain,
    IAuthMgrAuthorization::IAuthMgrAuthorization, SignedConnectionRequest::SignedConnectionRequest,
};
use binder::{ExceptionCode, Strong};
use hello_world_trusted_aidl::aidl::android::trusty::trustedhal::IHelloWorld::IHelloWorld;
use rpcbinder::RpcSession;
use tipc::Handle;

fn main() {
    trusty_log::init();
    match run() {
        Ok(()) => log::info!("bexos-authmgr-acceptance: complete"),
        Err(error) => panic!("bexos-authmgr-acceptance: {error}"),
    }
}

fn run() -> Result<(), String> {
    boot_ready::wait()?;
    let service: Strong<dyn IHelloWorld> =
        service_manager::wait_for_interface("android.trusty.trustedhal.IHelloWorld/default")
            .map_err(|e| format!("secure service connection failed: {e:?}"))?;
    let greeting = service
        .sayHello("BexOS")
        .map_err(|e| format!("authorized service invocation failed: {e:?}"))?;
    if greeting != "Hello BexOS" {
        return Err(format!("unexpected service reply: {greeting}"));
    }
    log::info!("bexos-authmgr-acceptance: authenticated connection accepted");

    // A live BE Binder session must explicitly reject malformed DICE. A
    // missing service, timeout, or transport failure does not pass this check.
    let handle = Handle::connect(c"com.android.trusty.rust.authmgr.V1")
        .map_err(|e| format!("negative-test BE connect failed: {e:?}"))?;
    handle
        .send(&(&[0u8][..]))
        .map_err(|e| format!("BE route selection: {e:?}"))?;
    let fd = handle.as_raw_fd();
    let session = RpcSession::new();
    let authorization: Strong<dyn IAuthMgrAuthorization> = session
        .setup_preconnected_client(move || Some(fd))
        .map_err(|e| format!("negative-test Binder setup: {e:?}"))?;
    match authorization.completeAuthentication(
        &SignedConnectionRequest {
            signedConnectionRequest: Vec::new(),
        },
        &DicePolicy {
            dicePolicy: Vec::new(),
        },
    ) {
        Err(status)
            if status.exception_code() == ExceptionCode::SERVICE_SPECIFIC
                && status.service_specific_error() == Error::AUTHENTICATION_NOT_STARTED.0 => {}
        other => {
            return Err(format!(
                "unauthenticated completion not rejected: {other:?}"
            ))
        }
    }
    log::info!("bexos-authmgr-acceptance: unauthenticated completion rejected");
    match authorization.initAuthentication(
        &ExplicitKeyDiceCertChain {
            diceCertChain: vec![0xff],
        },
        None,
    ) {
        Err(status)
            if status.exception_code() == ExceptionCode::SERVICE_SPECIFIC
                && status.service_specific_error() == Error::INVALID_DICE_CERT_CHAIN.0 => {}
        other => {
            return Err(format!(
                "malformed DICE was not explicitly rejected: {other:?}"
            ))
        }
    }
    log::info!("bexos-authmgr-acceptance: malformed DICE rejected");
    Ok(())
}
