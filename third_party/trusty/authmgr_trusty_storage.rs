// Copyright 2026 The BexOS Authors
// SPDX-License-Identifier: Apache-2.0

//! AuthMgr persistent-state adapter backed by Trusty's rollback-protected
//! storage service. This file is copied into the pinned upstream source tree
//! by `build_firmware.sh`; generated firmware remains a Bazel output.

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use authgraph_core::key::{InstanceIdentifier, Policy};
use authmgr_be::data_structures::{ClientId, PersistentClientContext, PersistentInstanceContext};
use authmgr_be::error::{Error, ErrorCode};
use authmgr_be::traits::{PersistentStorage, Storage};
use crate::authmgr_storage_format::{
    MAX_CONTEXT_BYTES, decode_context, decode_sequence, encode_context, encode_sequence,
    increment_sequence,
};
use std::sync::Mutex;
use storage::{Error as StorageError, ErrorCode as StorageErrorCode, Port, Session};

const SEQUENCE_PATH: &str = "authmgr.sequence";
const MAX_IDENTIFIER_BYTES: usize = 64;

pub struct TrustyPersistentStorage {
    session: Mutex<Session>,
}

impl TrustyPersistentStorage {
    pub fn new() -> Result<Self, Error> {
        let session = Session::new(Port::TamperDetectPersist, true)
            .map_err(|error| internal("connect to Trusty persistent storage", error))?;
        Ok(Self { session: Mutex::new(session) })
    }

    fn read_optional(&self, path: &str) -> Result<Option<Vec<u8>>, Error> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| Error(ErrorCode::InternalError, "Trusty storage mutex poisoned".into()))?;
        let mut bytes = vec![0u8; MAX_CONTEXT_BYTES];
        match session.read(path, &mut bytes) {
            Ok(read) => {
                let len = read.len();
                bytes.truncate(len);
                Ok(Some(bytes))
            }
            Err(StorageError::Code(StorageErrorCode::NotFound)) => Ok(None),
            Err(error) => Err(internal("read Trusty persistent state", error)),
        }
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        self.session
            .lock()
            .map_err(|_| Error(ErrorCode::InternalError, "Trusty storage mutex poisoned".into()))?
            .write(path, bytes)
            .map_err(|error| internal("write Trusty persistent state", error))
    }
}

impl Storage for TrustyPersistentStorage {
    type InstanceContext = PersistentInstanceContext;
    type ClientContext = PersistentClientContext;

    fn update_instance_policy_in_storage(
        &mut self,
        instance_id: &Arc<InstanceIdentifier>,
        latest_dice_policy: &Arc<Policy>,
    ) -> Result<(), Error> {
        let path = instance_path(instance_id)?;
        let mut context = self
            .read_instance_context(instance_id)?
            .ok_or_else(|| Error(ErrorCode::InternalError, "AuthMgr instance does not exist".into()))?;
        context.dice_policy = Arc::clone(latest_dice_policy);
        let encoded = encode_context(
            context.version,
            context.sequence_number,
            &context.dice_policy.0,
        )
        .map_err(|error| format_error("encode AuthMgr instance", error))?;
        self.write(&path, &encoded)
    }

    fn update_client_policy_in_storage(
        &mut self,
        instance_seq_number: i32,
        client_id: &Arc<ClientId>,
        latest_dice_policy: &Arc<Policy>,
    ) -> Result<(), Error> {
        let path = client_path(instance_seq_number, client_id)?;
        let mut context = self
            .read_client_context(instance_seq_number, client_id)?
            .ok_or_else(|| Error(ErrorCode::InternalError, "AuthMgr client does not exist".into()))?;
        context.dice_policy = Arc::clone(latest_dice_policy);
        let encoded = encode_context(
            context.version,
            context.sequence_number,
            &context.dice_policy.0,
        )
        .map_err(|error| format_error("encode AuthMgr client", error))?;
        self.write(&path, &encoded)
    }

    fn read_instance_context(
        &self,
        instance_id: &Arc<InstanceIdentifier>,
    ) -> Result<Option<Self::InstanceContext>, Error> {
        let Some(bytes) = self.read_optional(&instance_path(instance_id)?)? else {
            return Ok(None);
        };
        let (version, sequence_number, policy) =
            decode_context(&bytes).map_err(|error| format_error("decode AuthMgr instance", error))?;
        Ok(Some(PersistentInstanceContext {
            version,
            sequence_number,
            dice_policy: Arc::new(Policy(policy)),
        }))
    }

    fn create_instance_context(
        &mut self,
        instance_id: &Arc<InstanceIdentifier>,
        instance_info: Self::InstanceContext,
    ) -> Result<(), Error> {
        let path = instance_path(instance_id)?;
        if self.read_optional(&path)?.is_some() {
            return Err(Error(ErrorCode::InternalError, "AuthMgr instance already exists".into()));
        }
        self.write(
            &path,
            &encode_context(
                instance_info.version,
                instance_info.sequence_number,
                &instance_info.dice_policy.0,
            )
            .map_err(|error| format_error("encode AuthMgr instance", error))?,
        )
    }

    fn read_client_context(
        &self,
        instance_seq_number: i32,
        client_id: &Arc<ClientId>,
    ) -> Result<Option<Self::ClientContext>, Error> {
        let Some(bytes) = self.read_optional(&client_path(instance_seq_number, client_id)?)? else {
            return Ok(None);
        };
        let (version, sequence_number, policy) =
            decode_context(&bytes).map_err(|error| format_error("decode AuthMgr client", error))?;
        Ok(Some(PersistentClientContext {
            version,
            sequence_number,
            dice_policy: Arc::new(Policy(policy)),
        }))
    }

    fn create_client_context(
        &mut self,
        instance_seq_number: i32,
        client_id: &Arc<ClientId>,
        client_info: Self::ClientContext,
    ) -> Result<(), Error> {
        let path = client_path(instance_seq_number, client_id)?;
        if self.read_optional(&path)?.is_some() {
            return Err(Error(ErrorCode::InternalError, "AuthMgr client already exists".into()));
        }
        self.write(
            &path,
            &encode_context(
                client_info.version,
                client_info.sequence_number,
                &client_info.dice_policy.0,
            )
            .map_err(|error| format_error("encode AuthMgr client", error))?,
        )
    }
}

impl PersistentStorage for TrustyPersistentStorage {
    fn get_or_create_global_sequence_number(&mut self) -> Result<i32, Error> {
        match self.read_optional(SEQUENCE_PATH)? {
            Some(bytes) => decode_sequence(&bytes)
                .map_err(|error| format_error("decode AuthMgr sequence", error)),
            None => {
                self.write(SEQUENCE_PATH, &encode_sequence(0))?;
                Ok(0)
            }
        }
    }

    fn increment_global_sequence_number(&mut self) -> Result<i32, Error> {
        let current = self
            .read_optional(SEQUENCE_PATH)?
            .ok_or_else(|| Error(ErrorCode::InternalError, "AuthMgr sequence is missing".into()))?;
        let next = increment_sequence(&current)
            .map_err(|error| format_error("increment AuthMgr sequence", error))?;
        self.write(SEQUENCE_PATH, &next)?;
        decode_sequence(&next).map_err(|error| format_error("decode AuthMgr sequence", error))
    }
}

fn instance_path(instance_id: &[u8]) -> Result<String, Error> {
    Ok(format!("authmgr.i.{}", identifier_hex(instance_id)?))
}

fn client_path(instance_sequence: i32, client_id: &ClientId) -> Result<String, Error> {
    Ok(format!(
        "authmgr.c.{:08x}.{}",
        instance_sequence as u32,
        identifier_hex(&client_id.0)?
    ))
}

fn identifier_hex(identifier: &[u8]) -> Result<String, Error> {
    if identifier.is_empty() || identifier.len() > MAX_IDENTIFIER_BYTES {
        return Err(Error(ErrorCode::InternalError, "invalid AuthMgr persistent identifier".into()));
    }
    let mut out = String::with_capacity(identifier.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in identifier {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    Ok(out)
}

fn internal(context: &str, error: StorageError) -> Error {
    Error(ErrorCode::InternalError, format!("{context}: {error:?}"))
}

fn format_error(context: &str, error: crate::authmgr_storage_format::FormatError) -> Error {
    Error(ErrorCode::InternalError, format!("{context}: {error:?}"))
}
