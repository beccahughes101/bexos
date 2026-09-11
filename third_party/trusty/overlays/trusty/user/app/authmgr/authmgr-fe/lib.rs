/*
 * Copyright (C) 2025 The Android Open Source Project
 * Copyright 2026 The BexOS Authors
 * SPDX-License-Identifier: Apache-2.0
 */

mod accessor;

use alloc::rc::Rc;
use rpcbinder::RpcServer;
use std::ffi::CStr;
use tipc::{service_dispatcher, Manager, PortCfg, TipcError};

pub use accessor::{AuthMgrAccessor, SecurityConfig};

const SECURE_STORAGE_SERVICE_NAME: &str =
    "android.hardware.security.see.storage.ISecureStorage/default";
const SECURE_STORAGE_TARGET_PORT: &CStr = c"com.android.hardware.security.see.storage";
const HWKEY_SERVICE_NAME: &str = "android.hardware.security.see.hwcrypto.IHwCryptoKey/default";
const HWKEY_TARGET_PORT: &CStr = c"com.android.trusty.rust.hwcryptohal.V1";

const PORT_COUNT: usize = 2;
const CONNECTION_COUNT: usize = 6;

type AuthMgrAccessorService = rpcbinder::RpcServer;

service_dispatcher! {
    pub enum AuthMgrFeDispatcher {
        AuthMgrAccessorService,
    }
}

fn add_server_to_authmgr_dispatcher(
    dispatcher: &mut AuthMgrFeDispatcher<PORT_COUNT>,
    service_name: &'static str,
    target_port: &'static CStr,
) -> tipc::Result<()> {
    let accessor_server = RpcServer::new_per_session(move |uuid| {
        Some(
            AuthMgrAccessor::new_binder(
                service_name,
                get_security_config(target_port),
                uuid,
            )
            .as_binder(),
        )
    });
    let serving_port = service_manager::service_name_to_trusty_port(service_name)
        .map_err(|_| TipcError::InvalidPort)?;
    let service_cfg = PortCfg::new(serving_port)?.allow_ta_connect();
    dispatcher.add_service(Rc::new(accessor_server), service_cfg)
}

#[cfg(feature = "authmgrfe_mode_insecure")]
fn get_security_config(target_port: &'static CStr) -> SecurityConfig {
    log::warn!(
        "Using authmgr-fe SecurityConfig::Insecure - no authentication or authorization for trusted services."
    );
    SecurityConfig::Insecure { target_port }
}

#[cfg(not(feature = "authmgrfe_mode_insecure"))]
fn get_security_config(target_port: &'static CStr) -> SecurityConfig {
    log::info!("Using authmgr-fe SecurityConfig::Secure.");
    SecurityConfig::Secure { target_port }
}

pub fn init_and_start_loop() -> tipc::Result<()> {
    let mut dispatcher = AuthMgrFeDispatcher::<PORT_COUNT>::new()?;
    add_server_to_authmgr_dispatcher(
        &mut dispatcher,
        SECURE_STORAGE_SERVICE_NAME,
        SECURE_STORAGE_TARGET_PORT,
    )?;
    add_server_to_authmgr_dispatcher(&mut dispatcher, HWKEY_SERVICE_NAME, HWKEY_TARGET_PORT)?;
    Manager::<_, _, PORT_COUNT, CONNECTION_COUNT>::new_with_dispatcher(
        dispatcher,
        [],
    )?
    .run_event_loop()
}

#[cfg(test)]
mod tests {
    use super::*;
    use android_hardware_security_see_storage::aidl::android::hardware::security::see::storage::ISecureStorage::ISecureStorage;
    use binder::IBinder;
    use service_manager::*;
    use test::*;

    test::init!();

    #[test]
    fn test_get_secure_storage_binder() {
        let ss: Result<binder::Strong<dyn ISecureStorage>, binder::StatusCode> =
            wait_for_interface(SECURE_STORAGE_SERVICE_NAME);
        assert_ok!(ss.expect("secure storage interface to be resolved").as_binder().ping_binder());
    }
}
