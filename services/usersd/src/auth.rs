use crate::auth_transport::TeeClient;
use alloc::vec::Vec;
use bexos_trusty_client::protocol::{GATEKEEPER_UUID, HW_AUTH_TOKEN_TIMEOUT_SECS, KEYMINT_UUID};
use bexos_trusty_client::services::{
    GATEKEEPER_CMD_DELETE_USER, GATEKEEPER_CMD_ENROLL, GATEKEEPER_CMD_VERIFY, KEYMINT_CMD_BEGIN,
    KEYMINT_CMD_DELETE_KEY, KEYMINT_CMD_FINISH, KEYMINT_CMD_GENERATE_KEY, KeyMintAlgorithm,
    KeyMintPurpose, decode_gatekeeper_enroll, decode_gatekeeper_verify, decode_keymint_begin,
    decode_keymint_delete_key, decode_keymint_generated_key, decode_keymint_hmac_sha256,
    encode_gatekeeper_delete, encode_gatekeeper_enroll, encode_gatekeeper_reenroll,
    encode_gatekeeper_verify, encode_keymint_begin, encode_keymint_delete_key,
    encode_keymint_finish, encode_keymint_generate_auth_bound_key,
};
use bexos_trusty_client::users::{HardwareAuthToken, ukek_hmac_message};
use bexos_userspace::Channel;

use user_manager_fidl::UserStatus;

const UKEK_HMAC_ALIAS: &str = "bexos.usersd.ukek.hmac";

pub trait UserAuthProvider {
    fn enroll_password(&mut self, uid: u64, password: &str) -> Result<Enrollment, UserStatus>;
    fn verify_password(
        &mut self,
        uid: u64,
        secure_user_id: u64,
        password_handle: &[u8],
        password: &str,
    ) -> Result<HardwareAuthToken, UserStatus>;
    fn replace_password(
        &mut self,
        uid: u64,
        secure_user_id: u64,
        password_handle: &[u8],
        current_password: &str,
        new_password: &str,
    ) -> Result<Vec<u8>, UserStatus>;
    fn derive_ukek(
        &mut self,
        uid: u64,
        hmac_key_blob: &[u8],
        token: &HardwareAuthToken,
    ) -> Result<[u8; 32], UserStatus>;
    fn delete_user(
        &mut self,
        uid: u64,
        secure_user_id: u64,
        hmac_key_blob: &[u8],
    ) -> Result<(), UserStatus>;
    fn now_ms(&self) -> u64;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Enrollment {
    pub secure_user_id: u64,
    pub password_handle: Vec<u8>,
    pub hmac_key_blob: Vec<u8>,
}

pub enum RuntimeUserAuthProvider {
    Unsupported,
    Tee(TeeUserAuthProvider),
}

impl RuntimeUserAuthProvider {
    pub fn connect(manager: Channel) -> Result<Self, UserStatus> {
        Ok(Self::Tee(TeeUserAuthProvider::connect(manager)?))
    }
}

impl Default for RuntimeUserAuthProvider {
    fn default() -> Self {
        Self::Unsupported
    }
}

impl UserAuthProvider for RuntimeUserAuthProvider {
    fn enroll_password(&mut self, uid: u64, password: &str) -> Result<Enrollment, UserStatus> {
        match self {
            Self::Unsupported => Err(UserStatus::Storage),
            Self::Tee(provider) => provider.enroll_password(uid, password),
        }
    }

    fn replace_password(
        &mut self,
        uid: u64,
        secure_user_id: u64,
        password_handle: &[u8],
        current_password: &str,
        new_password: &str,
    ) -> Result<Vec<u8>, UserStatus> {
        match self {
            Self::Unsupported => Err(UserStatus::Storage),
            Self::Tee(provider) => provider.replace_password(
                uid,
                secure_user_id,
                password_handle,
                current_password,
                new_password,
            ),
        }
    }

    fn verify_password(
        &mut self,
        uid: u64,
        secure_user_id: u64,
        password_handle: &[u8],
        password: &str,
    ) -> Result<HardwareAuthToken, UserStatus> {
        match self {
            Self::Unsupported => Err(UserStatus::Storage),
            Self::Tee(provider) => {
                provider.verify_password(uid, secure_user_id, password_handle, password)
            }
        }
    }

    fn derive_ukek(
        &mut self,
        uid: u64,
        hmac_key_blob: &[u8],
        token: &HardwareAuthToken,
    ) -> Result<[u8; 32], UserStatus> {
        match self {
            Self::Unsupported => Err(UserStatus::Storage),
            Self::Tee(provider) => provider.derive_ukek(uid, hmac_key_blob, token),
        }
    }

    fn delete_user(
        &mut self,
        uid: u64,
        secure_user_id: u64,
        hmac_key_blob: &[u8],
    ) -> Result<(), UserStatus> {
        match self {
            Self::Unsupported => Err(UserStatus::Storage),
            Self::Tee(provider) => provider.delete_user(uid, secure_user_id, hmac_key_blob),
        }
    }

    fn now_ms(&self) -> u64 {
        match self {
            Self::Unsupported => 0,
            Self::Tee(provider) => provider.now_ms(),
        }
    }
}

pub struct TeeUserAuthProvider {
    client: TeeClient,
    gatekeeper_session: Option<u64>,
    keymint_session: Option<u64>,
}

impl TeeUserAuthProvider {
    pub fn connect(client: Channel) -> Result<Self, UserStatus> {
        // Appd already bound this restricted endpoint to teed. Another binding
        // request would send metadata into the FIDL request queue.
        if client.0 == 0 {
            return Err(UserStatus::Storage);
        }
        Ok(Self {
            client: TeeClient::new(client, false),
            gatekeeper_session: None,
            keymint_session: None,
        })
    }

    pub fn from_parts(
        client: Channel,
        gatekeeper_session: Option<u64>,
        keymint_session: Option<u64>,
        awaiting_response: bool,
    ) -> Self {
        Self {
            client: TeeClient::new(client, awaiting_response),
            gatekeeper_session,
            keymint_session,
        }
    }

    pub fn client(&self) -> Channel {
        self.client.channel()
    }

    pub fn awaiting_response(&self) -> bool {
        self.client.awaiting_response()
    }

    pub fn gatekeeper_session(&self) -> Option<u64> {
        self.gatekeeper_session
    }

    pub fn keymint_session(&self) -> Option<u64> {
        self.keymint_session
    }

    fn open_gatekeeper(&mut self) -> Result<u64, UserStatus> {
        if let Some(session) = self.gatekeeper_session {
            return Ok(session);
        }
        let session = self.client.open_session(GATEKEEPER_UUID)?;
        self.gatekeeper_session = Some(session);
        Ok(session)
    }

    fn open_keymint(&mut self) -> Result<u64, UserStatus> {
        if let Some(session) = self.keymint_session {
            return Ok(session);
        }
        let session = self.client.open_session(KEYMINT_UUID)?;
        self.keymint_session = Some(session);
        Ok(session)
    }
}

impl UserAuthProvider for TeeUserAuthProvider {
    fn enroll_password(&mut self, uid: u64, password: &str) -> Result<Enrollment, UserStatus> {
        let gatekeeper = self.open_gatekeeper()?;
        let enroll_payload =
            encode_gatekeeper_enroll(uid, password).map_err(|_| UserStatus::InvalidArgs)?;
        let enrollment = decode_gatekeeper_enroll(&self.client.invoke(
            gatekeeper,
            GATEKEEPER_CMD_ENROLL,
            &enroll_payload,
        )?)
        .map_err(|error| {
            bexos_userspace::log(&alloc::format!(
                "usersd: Gatekeeper enrollment response failed: {error:?}\n"
            ));
            UserStatus::Storage
        })?;

        let keymint = match self.open_keymint() {
            Ok(session) => session,
            Err(status) => {
                self.delete_gatekeeper_enrollment(uid, enrollment.secure_user_id);
                return Err(status);
            }
        };
        let key_payload = match encode_keymint_generate_auth_bound_key(
            uid,
            UKEK_HMAC_ALIAS,
            KeyMintAlgorithm::HmacSha256,
            enrollment.secure_user_id,
            HW_AUTH_TOKEN_TIMEOUT_SECS,
        ) {
            Ok(payload) => payload,
            Err(_) => {
                self.delete_gatekeeper_enrollment(uid, enrollment.secure_user_id);
                return Err(UserStatus::Storage);
            }
        };
        let generated_result = self
            .client
            .invoke(keymint, KEYMINT_CMD_GENERATE_KEY, &key_payload)
            .and_then(|bytes| {
                decode_keymint_generated_key(&bytes).map_err(|error| {
                    bexos_userspace::log(&alloc::format!(
                        "usersd: auth-bound KeyMint generation failed: {error:?}\n"
                    ));
                    UserStatus::Storage
                })
            });
        let generated = match generated_result {
            Ok(generated) => generated,
            Err(status) => {
                self.delete_gatekeeper_enrollment(uid, enrollment.secure_user_id);
                return Err(status);
            }
        };
        if let Err(status) = self.verify_password(
            uid,
            enrollment.secure_user_id,
            &enrollment.password_handle,
            password,
        ) {
            self.delete_keymint_blob(&generated.opaque_blob);
            self.delete_gatekeeper_enrollment(uid, enrollment.secure_user_id);
            return Err(status);
        }

        Ok(Enrollment {
            secure_user_id: enrollment.secure_user_id,
            password_handle: enrollment.password_handle,
            hmac_key_blob: generated.opaque_blob,
        })
    }

    fn verify_password(
        &mut self,
        uid: u64,
        secure_user_id: u64,
        password_handle: &[u8],
        password: &str,
    ) -> Result<HardwareAuthToken, UserStatus> {
        let gatekeeper = self.open_gatekeeper()?;
        let payload = encode_gatekeeper_verify(uid, secure_user_id, password_handle, password)
            .map_err(|_| UserStatus::InvalidArgs)?;
        let verified = decode_gatekeeper_verify(&self.client.invoke(
            gatekeeper,
            GATEKEEPER_CMD_VERIFY,
            &payload,
        )?)
        .map_err(|_| UserStatus::AccessDenied)?;
        Ok(HardwareAuthToken::new(
            uid,
            verified.secure_user_id,
            verified.secure_timestamp_ms,
            verified.auth_token,
        ))
    }

    fn replace_password(
        &mut self,
        uid: u64,
        secure_user_id: u64,
        password_handle: &[u8],
        current_password: &str,
        new_password: &str,
    ) -> Result<Vec<u8>, UserStatus> {
        let gatekeeper = self.open_gatekeeper()?;
        let payload =
            encode_gatekeeper_reenroll(uid, password_handle, current_password, new_password)
                .map_err(|_| UserStatus::InvalidArgs)?;
        let enrollment = decode_gatekeeper_enroll(&self.client.invoke(
            gatekeeper,
            GATEKEEPER_CMD_ENROLL,
            &payload,
        )?)
        .map_err(|_| UserStatus::AccessDenied)?;
        if enrollment.secure_user_id != secure_user_id {
            return Err(UserStatus::BadState);
        }
        Ok(enrollment.password_handle)
    }

    fn derive_ukek(
        &mut self,
        uid: u64,
        hmac_key_blob: &[u8],
        token: &HardwareAuthToken,
    ) -> Result<[u8; 32], UserStatus> {
        let keymint = self.open_keymint()?;
        let begin = encode_keymint_begin(
            KeyMintAlgorithm::HmacSha256,
            KeyMintPurpose::Sign,
            hmac_key_blob,
            None,
            Some(&token.encoded),
        )
        .map_err(|_| UserStatus::Storage)?;
        let begun =
            decode_keymint_begin(&self.client.invoke(keymint, KEYMINT_CMD_BEGIN, &begin)?)
                .map_err(|_| UserStatus::AccessDenied)?;
        let finish =
            encode_keymint_finish(begun.handle, &ukek_hmac_message(uid), Some(&token.encoded))
                .map_err(|_| UserStatus::Storage)?;
        decode_keymint_hmac_sha256(&self.client.invoke(keymint, KEYMINT_CMD_FINISH, &finish)?)
            .map_err(|_| UserStatus::AccessDenied)
    }

    fn delete_user(
        &mut self,
        uid: u64,
        secure_user_id: u64,
        hmac_key_blob: &[u8],
    ) -> Result<(), UserStatus> {
        let keymint = self.open_keymint()?;
        let key_payload =
            encode_keymint_delete_key(hmac_key_blob).map_err(|_| UserStatus::InvalidArgs)?;
        let response = self
            .client
            .invoke(keymint, KEYMINT_CMD_DELETE_KEY, &key_payload)?;
        decode_keymint_delete_key(&response).map_err(|_| UserStatus::Storage)?;
        let gatekeeper = self.open_gatekeeper()?;
        let payload =
            encode_gatekeeper_delete(uid, secure_user_id).map_err(|_| UserStatus::InvalidArgs)?;
        self.client
            .invoke(gatekeeper, GATEKEEPER_CMD_DELETE_USER, &payload)?;
        Ok(())
    }

    fn now_ms(&self) -> u64 {
        bexos_userspace::live_migration::now_ms()
    }
}

impl TeeUserAuthProvider {
    fn delete_keymint_blob(&mut self, key_blob: &[u8]) {
        let Ok(keymint) = self.open_keymint() else {
            return;
        };
        let Ok(payload) = encode_keymint_delete_key(key_blob) else {
            return;
        };
        let _ = self
            .client
            .invoke(keymint, KEYMINT_CMD_DELETE_KEY, &payload);
    }

    fn delete_gatekeeper_enrollment(&mut self, uid: u64, secure_user_id: u64) {
        let Ok(gatekeeper) = self.open_gatekeeper() else {
            return;
        };
        let Ok(payload) = encode_gatekeeper_delete(uid, secure_user_id) else {
            return;
        };
        let _ = self
            .client
            .invoke(gatekeeper, GATEKEEPER_CMD_DELETE_USER, &payload);
    }
}
