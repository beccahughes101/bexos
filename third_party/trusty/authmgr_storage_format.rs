// Copyright 2026 The BexOS Authors
// SPDX-License-Identifier: Apache-2.0

//! Bounded on-disk encoding used by the Trusty AuthMgr storage adapter.

extern crate alloc;

use alloc::vec::Vec;

const CONTEXT_MAGIC: &[u8; 4] = b"AMC1";
const SEQUENCE_MAGIC: &[u8; 4] = b"AMS1";
pub const MAX_CONTEXT_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormatError {
    ContextHeader,
    ContextLength,
    PolicyTooLarge,
    Sequence,
    SequenceOverflow,
}

pub fn encode_context(version: i32, sequence: i32, policy: &[u8]) -> Result<Vec<u8>, FormatError> {
    let policy_len = u32::try_from(policy.len()).map_err(|_| FormatError::PolicyTooLarge)?;
    if policy.len() > MAX_CONTEXT_BYTES - 16 {
        return Err(FormatError::PolicyTooLarge);
    }
    let mut out = Vec::with_capacity(16 + policy.len());
    out.extend_from_slice(CONTEXT_MAGIC);
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&policy_len.to_le_bytes());
    out.extend_from_slice(policy);
    Ok(out)
}

pub fn decode_context(bytes: &[u8]) -> Result<(i32, i32, Vec<u8>), FormatError> {
    if bytes.len() < 16 || &bytes[..4] != CONTEXT_MAGIC {
        return Err(FormatError::ContextHeader);
    }
    let version = i32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let sequence = i32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let policy_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    if policy_len > MAX_CONTEXT_BYTES - 16 || bytes.len() != 16 + policy_len {
        return Err(FormatError::ContextLength);
    }
    Ok((version, sequence, bytes[16..].to_vec()))
}

pub fn encode_sequence(sequence: i32) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(SEQUENCE_MAGIC);
    out[4..].copy_from_slice(&sequence.to_le_bytes());
    out
}

pub fn decode_sequence(bytes: &[u8]) -> Result<i32, FormatError> {
    if bytes.len() != 8 || &bytes[..4] != SEQUENCE_MAGIC {
        return Err(FormatError::Sequence);
    }
    Ok(i32::from_le_bytes(bytes[4..].try_into().unwrap()))
}

pub fn increment_sequence(bytes: &[u8]) -> Result<[u8; 8], FormatError> {
    let current = decode_sequence(bytes)?;
    let next = current
        .checked_add(1)
        .ok_or(FormatError::SequenceOverflow)?;
    Ok(encode_sequence(next))
}
