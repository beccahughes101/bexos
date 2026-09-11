use alloc::vec;
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
use bexos_userspace::{Channel, Memory, Rpc};
use tee_manager_fidl as tee;
use tee_manager_fidl::{FidlDecode as TeeFidlDecode, FidlEncode as TeeFidlEncode};

use user_manager_fidl::UserStatus;

const UKEK_HMAC_ALIAS: &str = "bexos.usersd.ukek.hmac";
// Teed may continue a Trusty standard call while secure-world storage or a
// previous interrupt is being serviced. Keep the usersd envelope alive for
// the same durable-operation window used by its callers so an eventual reply
// cannot be mistaken for the next authentication request.
const TEE_RPC_TIMEOUT_SECONDS: u64 = 360;

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

#[derive(Clone, Copy)]
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

#[derive(Clone, Copy)]
pub struct TeeUserAuthProvider {
    client: Channel,
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
            client,
            gatekeeper_session: None,
            keymint_session: None,
        })
    }

    pub fn from_parts(
        client: Channel,
        gatekeeper_session: Option<u64>,
        keymint_session: Option<u64>,
    ) -> Self {
        Self {
            client,
            gatekeeper_session,
            keymint_session,
        }
    }

    pub fn client(&self) -> Channel {
        self.client
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
        let session = tee_open_session(self.client, GATEKEEPER_UUID)?;
        self.gatekeeper_session = Some(session);
        Ok(session)
    }

    fn open_keymint(&mut self) -> Result<u64, UserStatus> {
        if let Some(session) = self.keymint_session {
            return Ok(session);
        }
        let session = tee_open_session(self.client, KEYMINT_UUID)?;
        self.keymint_session = Some(session);
        Ok(session)
    }
}

impl UserAuthProvider for TeeUserAuthProvider {
    fn enroll_password(&mut self, uid: u64, password: &str) -> Result<Enrollment, UserStatus> {
        let gatekeeper = self.open_gatekeeper()?;
        let enroll_payload =
            encode_gatekeeper_enroll(uid, password).map_err(|_| UserStatus::InvalidArgs)?;
        let enrollment = decode_gatekeeper_enroll(&tee_invoke(
            self.client,
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
        let generated_result =
            tee_invoke(self.client, keymint, KEYMINT_CMD_GENERATE_KEY, &key_payload).and_then(
                |bytes| {
                    decode_keymint_generated_key(&bytes).map_err(|error| {
                        bexos_userspace::log(&alloc::format!(
                            "usersd: auth-bound KeyMint generation failed: {error:?}\n"
                        ));
                        UserStatus::Storage
                    })
                },
            );
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
        let verified = decode_gatekeeper_verify(&tee_invoke(
            self.client,
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
        let enrollment = decode_gatekeeper_enroll(&tee_invoke(
            self.client,
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
        let begun = decode_keymint_begin(&tee_invoke(
            self.client,
            keymint,
            KEYMINT_CMD_BEGIN,
            &begin,
        )?)
        .map_err(|_| UserStatus::AccessDenied)?;
        let finish =
            encode_keymint_finish(begun.handle, &ukek_hmac_message(uid), Some(&token.encoded))
                .map_err(|_| UserStatus::Storage)?;
        decode_keymint_hmac_sha256(&tee_invoke(
            self.client,
            keymint,
            KEYMINT_CMD_FINISH,
            &finish,
        )?)
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
        let response = tee_invoke(self.client, keymint, KEYMINT_CMD_DELETE_KEY, &key_payload)?;
        decode_keymint_delete_key(&response).map_err(|_| UserStatus::Storage)?;
        let gatekeeper = self.open_gatekeeper()?;
        let payload =
            encode_gatekeeper_delete(uid, secure_user_id).map_err(|_| UserStatus::InvalidArgs)?;
        tee_invoke(
            self.client,
            gatekeeper,
            GATEKEEPER_CMD_DELETE_USER,
            &payload,
        )?;
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
        let _ = tee_invoke(self.client, keymint, KEYMINT_CMD_DELETE_KEY, &payload);
    }

    fn delete_gatekeeper_enrollment(&mut self, uid: u64, secure_user_id: u64) {
        let Ok(gatekeeper) = self.open_gatekeeper() else {
            return;
        };
        let Ok(payload) = encode_gatekeeper_delete(uid, secure_user_id) else {
            return;
        };
        let _ = tee_invoke(
            self.client,
            gatekeeper,
            GATEKEEPER_CMD_DELETE_USER,
            &payload,
        );
    }
}

fn tee_open_session(client: Channel, uuid: [u8; 16]) -> Result<u64, UserStatus> {
    let response = tee_call(client, 5, &tee::TeeManagerOpenSessionRequest { uuid })?;
    let decoded = tee::TeeManagerOpenSessionResponse::decode(&response.bytes, &response.handles)
        .map_err(|_| UserStatus::Storage)?;
    if decoded.status != tee::TeeStatus::Ok {
        return Err(UserStatus::Storage);
    }
    Ok(decoded.session_id)
}

fn tee_invoke(
    client: Channel,
    session_id: u64,
    command_id: u32,
    payload: &[u8],
) -> Result<Vec<u8>, UserStatus> {
    let payload_vmo = Memory::from_bytes(payload).map_err(|_| UserStatus::Storage)?;
    let call = tee_call(
        client,
        7,
        &tee::TeeManagerInvokeCommandRequest {
            session_id,
            command_id,
            payload: tee::HandleRef { raw: payload_vmo },
            payload_len: payload.len() as u64,
        },
    );
    let response = call?;
    let decoded =
        match tee::TeeManagerInvokeCommandResponse::decode(&response.bytes, &response.handles) {
            Ok(decoded) => decoded,
            Err(_) => {
                for handle in response.handles {
                    let _ = Memory::close(handle.raw);
                }
                return Err(UserStatus::Storage);
            }
        };
    if decoded.status != tee::TeeStatus::Ok {
        let _ = Memory::close(decoded.response.raw);
        return Err(UserStatus::AccessDenied);
    }
    if decoded.response_len == 0 || decoded.response_len > 64 * 1024 {
        let _ = Memory::close(decoded.response.raw);
        return Err(UserStatus::Storage);
    }
    let rounded = decoded
        .response_len
        .checked_add(4095)
        .map(|len| len & !4095)
        .ok_or(UserStatus::Storage)?;
    let va = Memory::map(decoded.response.raw, rounded, 2).map_err(|_| UserStatus::Storage)?;
    let bytes =
        unsafe { core::slice::from_raw_parts(va as *const u8, decoded.response_len as usize) }
            .to_vec();
    Memory::unmap(va, rounded).map_err(|_| UserStatus::Storage)?;
    Memory::close(decoded.response.raw).map_err(|_| UserStatus::Storage)?;
    Ok(bytes)
}

struct TeeResponse {
    bytes: Vec<u8>,
    handles: Vec<tee::HandleRef>,
}

fn tee_call<Q: TeeFidlEncode>(
    channel: Channel,
    ordinal: u64,
    request: &Q,
) -> Result<TeeResponse, UserStatus> {
    let mut bytes = vec![0; 65500];
    let mut handles = [tee::HandleRef { raw: 0 }; 8];
    let encoded = request
        .encode(&mut bytes, &mut handles)
        .map_err(|_| UserStatus::Storage)?;
    let message = Rpc(channel)
        .call_raw_with_timeout(
            ordinal,
            &bytes[..encoded.bytes],
            &handles[..encoded.handles]
                .iter()
                .map(|handle| handle.raw)
                .collect::<Vec<_>>(),
            true,
            TEE_RPC_TIMEOUT_SECONDS,
        )
        .map_err(|error| {
            bexos_userspace::log(&alloc::format!(
                "usersd: TEE transport failed ordinal={ordinal} error={error:?}\n"
            ));
            UserStatus::Storage
        })?;
    Ok(TeeResponse {
        bytes: message.bytes,
        handles: message
            .handles
            .iter()
            .map(|raw| tee::HandleRef { raw: *raw })
            .collect(),
    })
}
