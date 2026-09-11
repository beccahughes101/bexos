//! Root-only x86 firmware selection. A failed mutation exchange is always
//! indeterminate until a subsequent authenticated read resolves its outcome.
use crate::ql::Transport;
pub use bexos_secure_firmware::selection::{Identity, Operation, Phase, Slot, State};
use bexos_secure_firmware::selection::{
    REQUEST_BYTES, STATE_BYTES, query_request, validate_mutation_request,
};
pub const OPERATION: u32 = 0x8000_0002;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Unavailable,
    Invalid,
    Uncertain,
}
pub fn query(transport: &mut impl Transport) -> Result<State, Error> {
    let mut b = [0; STATE_BYTES];
    b[..REQUEST_BYTES].copy_from_slice(&query_request());
    if transport
        .exchange(OPERATION, REQUEST_BYTES, &mut b)
        .map_err(|_| Error::Unavailable)?
        != STATE_BYTES as i64
    {
        return Err(Error::Unavailable);
    }
    State::decode(&b).map_err(|_| Error::Invalid)
}
/// The request must be produced by the last authenticated State. Retain these
/// exact bytes across retry; constructing a new request could repeat a trial.
pub fn mutate(
    transport: &mut impl Transport,
    request: &[u8; REQUEST_BYTES],
) -> Result<State, Error> {
    validate_mutation_request(request).map_err(|_| Error::Invalid)?;
    let mut b = [0; STATE_BYTES];
    b[..REQUEST_BYTES].copy_from_slice(request);
    if transport
        .exchange(OPERATION, REQUEST_BYTES, &mut b)
        .map_err(|_| Error::Uncertain)?
        != STATE_BYTES as i64
    {
        return Err(Error::Uncertain);
    }
    let state = State::decode(&b).map_err(|_| Error::Uncertain)?;
    if !state.acknowledges(request) {
        return Err(Error::Uncertain);
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Lost;
    impl Transport for Lost {
        fn exchange(
            &mut self,
            op: u32,
            length: usize,
            b: &mut [u8],
        ) -> Result<i64, crate::ql::Error> {
            assert_eq!(
                (op, length, b.len()),
                (OPERATION, REQUEST_BYTES, STATE_BYTES)
            );
            Err(crate::ql::Error::Timeout)
        }
        fn now_ns(&self) -> u64 {
            0
        }
    }
    #[test]
    fn publication_failure_cannot_authorize_rollback() {
        let r = bexos_secure_firmware::selection::request(
            1,
            Operation::Commit,
            bexos_secure_firmware::Component::Hypervisor,
            Identity {
                slot: Slot::B,
                generation: 2,
                digest: [0x5a; 32],
                length: 256,
            },
        )
        .unwrap();
        assert_eq!(mutate(&mut Lost, &r), Err(Error::Uncertain));
        assert_eq!(query(&mut Lost), Err(Error::Unavailable));
    }
}
