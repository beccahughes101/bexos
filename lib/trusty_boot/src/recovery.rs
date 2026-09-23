//! Serialized architecture-bound firmware installation and authenticated reboot decisions.
//! This module never treats an unavailable reply as permission to roll back.
//! Its caller owns the recovery domain, firmware disk and client-admission gate.
use crate::{ql::Transport, selection};
use bexos_secure_firmware::{
    Component,
    selection::{Identity, MAX_ATTEMPTS, Operation, Phase, State},
    store::{self, BlockDevice},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Journal,
    Conflict,
    Busy,
    Storage(store::Error),
    CommittedUnavailable(Component),
    InvalidTrial,
}

/// A decision can only be made from a fresh authenticated read. In particular,
/// a caller cannot resume a cached pending trial after losing a commit reply.
#[derive(Debug)]
pub struct Boot {
    state: State,
    selected: [Identity; 2],
    rolled_back: bool,
}
impl Boot {
    /// The rollback image is the authenticated committed identity, which can
    /// differ from both the selected trial and the embedded recovery baseline.
    pub fn committed(&self, component: Component) -> Identity {
        self.state.committed(component)
    }
    pub fn selected(&self, component: Component) -> Identity {
        self.selected[index(component)]
    }
    pub fn trial(&self) -> Option<(Component, Identity)> {
        self.state.pending
    }
    pub fn rolled_back(&self) -> bool {
        self.rolled_back
    }
    /// Canonical commitment ticket for the resident handoff. It identifies the
    /// exact trial and revision, and grants no authority without the protected
    /// journal agreeing when the execution owner completes its health checks.
    pub fn commit_request(&self) -> Option<[u8; 256]> {
        let (component, image) = self.trial()?;
        self.state.request(Operation::Commit, component, image).ok()
    }
    /// The execution owner must keep persistent service mutations fenced and
    /// normal clients excluded until this trial has passed its health checks.
    /// Consume the decision so it cannot be committed twice or after abort.
    pub fn commit(self, transport: &mut impl Transport) -> Result<State, Error> {
        let (component, image) = self.trial().ok_or(Error::InvalidTrial)?;
        publish(transport, &self.state, Operation::Commit, component, image)
    }
    pub fn abort(self, transport: &mut impl Transport) -> Result<State, Error> {
        let (component, image) = self.trial().ok_or(Error::InvalidTrial)?;
        publish(transport, &self.state, Operation::Abort, component, image)
    }
}

/// Complete a trial whose ticket crossed a resident image handoff. Always read
/// first: an already committed ticket succeeds without replaying the mutation.
/// The execution owner calls this only after fenced trial health succeeds.
pub fn commit_prepared(
    transport: &mut impl Transport,
    request: &[u8; 256],
) -> Result<State, Error> {
    use bexos_secure_firmware::selection::{component, validate_mutation_request};
    validate_mutation_request(request).map_err(|_| Error::InvalidTrial)?;
    if request[8..12] != Operation::Commit.number().to_le_bytes() {
        return Err(Error::InvalidTrial);
    }
    let architecture = bexos_secure_firmware::Architecture::from_number(u32::from_le_bytes(
        request[12..16].try_into().unwrap(),
    ))
    .ok_or(Error::InvalidTrial)?;
    let state = selection::query_for(transport, architecture).map_err(|_| Error::Journal)?;
    if state.acknowledges(request) {
        return Ok(state);
    }
    let component = component(u32::from_le_bytes(request[24..28].try_into().unwrap()))
        .map_err(|_| Error::InvalidTrial)?;
    let image = Identity::decode(&request[32..88], false).map_err(|_| Error::InvalidTrial)?;
    if state
        .request(Operation::Commit, component, image)
        .map_err(|_| Error::Conflict)?
        != *request
    {
        return Err(Error::Conflict);
    }
    publish(transport, &state, Operation::Commit, component, image)
}

/// Start an already authenticated and prepared live trial. The permanent
/// execution owner retains the source component until health and commitment.
pub fn begin_live(
    transport: &mut impl Transport,
    component: Component,
    image: Identity,
) -> Result<Boot, Error> {
    let before = selection::query(transport).map_err(|_| Error::Journal)?;
    if before.phase != Phase::Pending
        || before.pending != Some((component, image))
        || before.attempts >= MAX_ATTEMPTS
    {
        return Err(Error::Conflict);
    }
    let state = publish(transport, &before, Operation::Attempt, component, image)?;
    let mut selected = state.committed;
    selected[index(component)] = image;
    Ok(Boot {
        state,
        selected,
        rolled_back: false,
    })
}

pub fn begin_live_monitor(transport: &mut impl Transport, image: Identity) -> Result<Boot, Error> {
    begin_live(transport, Component::Hypervisor, image)
}

/// Resolve a mutation with authenticated reads and, at most, one exact retry.
/// A read showing unrelated progress is a conflict, never an implicit abort.
fn publish(
    transport: &mut impl Transport,
    before: &State,
    op: Operation,
    component: Component,
    image: Identity,
) -> Result<State, Error> {
    let request = before
        .request(op, component, image)
        .map_err(|_| Error::Conflict)?;
    for attempt in 0..2 {
        if let Ok(state) = selection::mutate(transport, &request) {
            return Ok(state);
        }
        let observed =
            selection::query_for(transport, before.architecture()).map_err(|_| Error::Journal)?;
        if observed.acknowledges(&request) {
            return Ok(observed);
        }
        if observed != *before {
            return Err(Error::Conflict);
        }
        if attempt == 1 {
            return Err(Error::Journal);
        }
    }
    unreachable!()
}

/// Publish pending only after flush, reread, and signature verification of the
/// inactive slot. The execution owner must serialize this whole operation.
pub fn stage(
    transport: &mut impl Transport,
    device: &mut impl BlockDevice,
    component: Component,
    bundle: &[u8],
    root: &[u8],
    floor: u64,
    scratch: &mut [u8],
) -> Result<State, Error> {
    let before =
        selection::query_for(transport, device.architecture()).map_err(|_| Error::Journal)?;
    if before.phase != Phase::Idle {
        return Err(Error::Busy);
    }
    let image = store::install(
        device,
        component,
        before.committed(component),
        bundle,
        root,
        floor,
        scratch,
    )
    .map_err(Error::Storage)?;
    publish(transport, &before, Operation::Stage, component, image)
}

/// Resident installation with one bounded upload/work allocation. As with
/// `stage`, no pending record is published until the slot reread authenticates.
pub fn stage_in_place(
    transport: &mut impl Transport,
    device: &mut impl BlockDevice,
    component: Component,
    bundle: &mut [u8],
    root: &[u8],
    floor: u64,
) -> Result<State, Error> {
    let before =
        selection::query_for(transport, device.architecture()).map_err(|_| Error::Journal)?;
    if before.phase != Phase::Idle {
        return Err(Error::Busy);
    }
    let image = store::install_in_place(
        device,
        component,
        before.committed(component),
        bundle,
        root,
        floor,
    )
    .map_err(Error::Storage)?;
    publish(transport, &before, Operation::Stage, component, image)
}

/// Authenticate every committed component before considering any trial. If a
/// committed image is missing, do not substitute even a valid older image.
/// INITIAL denotes the exact signed recovery bootstrap's embedded generation.
/// A boot decision identifies images; execution must reload and authenticate
/// those exact identities, since the disk itself is not authoritative.
pub fn prepare_boot(
    transport: &mut impl Transport,
    device: &mut impl BlockDevice,
    root: &[u8],
    floors: [u64; 2],
    scratch: &mut [u8],
) -> Result<Boot, Error> {
    let mut state =
        selection::query_for(transport, device.architecture()).map_err(|_| Error::Journal)?;
    for component in [Component::Trusty, Component::Hypervisor] {
        let committed = state.committed(component);
        if committed.generation < floors[index(component)]
            || (committed != Identity::INITIAL
                && store::load(
                    device,
                    component,
                    committed,
                    root,
                    floors[index(component)],
                    scratch,
                )
                .is_err())
        {
            return Err(Error::CommittedUnavailable(component));
        }
    }
    let mut selected = state.committed;
    let mut rolled_back = false;
    if let Some((component, image)) = state.pending {
        if state.attempts >= MAX_ATTEMPTS
            || store::load(
                device,
                component,
                image,
                root,
                floors[index(component)],
                scratch,
            )
            .is_err()
        {
            state = publish(transport, &state, Operation::Abort, component, image)?;
            rolled_back = true;
        } else {
            state = publish(transport, &state, Operation::Attempt, component, image)?;
            selected[index(component)] = image;
        }
    }
    Ok(Boot {
        state,
        selected,
        rolled_back,
    })
}

const fn index(component: Component) -> usize {
    match component {
        Component::Trusty => 0,
        Component::Hypervisor => 1,
    }
}
