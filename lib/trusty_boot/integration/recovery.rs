use bexos_secure_firmware::{
    Component,
    selection::{Identity, Phase, State},
    store::{self, BlockDevice},
};
use bexos_trusty_boot::{ql, recovery, selection};
use std::{
    collections::BTreeMap,
    io::Write,
    process::{Command, Stdio},
};

const BUNDLE: &[u8] = include_bytes!(env!("TEST_FIRMWARE"));
const ROOT: &[u8] = include_bytes!(env!("TEST_ROOT"));
const RECORDS: &[u8] = include_bytes!(env!("SELECTION_RECORDS"));
const COMPONENT: Component = Component::Hypervisor;

struct Authority {
    bytes: [u8; 512],
    lose_mutation_reply: bool,
    lose_query_reply: bool,
    drop_mutation: bool,
    exchanges: usize,
}
impl Authority {
    fn new() -> Self {
        Self {
            bytes: RECORDS[..512].try_into().unwrap(),
            lose_mutation_reply: false,
            lose_query_reply: false,
            drop_mutation: false,
            exchanges: 0,
        }
    }
    fn state(&self) -> State {
        State::decode(&self.bytes).unwrap()
    }
}
impl ql::Transport for Authority {
    fn exchange(&mut self, op: u32, len: usize, bytes: &mut [u8]) -> Result<i64, ql::Error> {
        assert_eq!((op, len, bytes.len()), (selection::OPERATION, 256, 512));
        self.exchanges += 1;
        let query = bytes[8] == 1;
        if query && self.lose_query_reply || !query && self.drop_mutation {
            return Err(ql::Error::Timeout);
        }
        let mut child = Command::new(env!("SELECTION_AUTHORITY"))
            .arg("--step")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(&self.bytes).unwrap();
        input.write_all(&bytes[..256]).unwrap();
        drop(input);
        let result = child.wait_with_output().unwrap();
        if !result.status.success() {
            return Err(ql::Error::Rejected);
        }
        self.bytes.copy_from_slice(&result.stdout);
        if !query && self.lose_mutation_reply {
            return Err(ql::Error::Timeout);
        }
        bytes.copy_from_slice(&self.bytes);
        Ok(512)
    }
    fn now_ns(&self) -> u64 {
        0
    }
}

#[derive(Default)]
struct Disk {
    architecture: Option<bexos_secure_firmware::Architecture>,
    data: BTreeMap<u64, [u8; 512]>,
    fail: bool,
    writes: usize,
}
impl BlockDevice for Disk {
    fn architecture(&self) -> bexos_secure_firmware::Architecture {
        self.architecture
            .unwrap_or(bexos_secure_firmware::Architecture::X86_64)
    }
    fn sectors(&self) -> u64 {
        store::DISK_BYTES / 512
    }
    fn read(&mut self, sector: u64, bytes: &mut [u8; 512]) -> Result<(), store::Error> {
        if self.fail {
            return Err(store::Error::Device);
        }
        *bytes = *self.data.get(&sector).unwrap_or(&[0; 512]);
        Ok(())
    }
    fn write(&mut self, sector: u64, bytes: &[u8; 512]) -> Result<(), store::Error> {
        if self.fail {
            return Err(store::Error::Device);
        }
        self.writes += 1;
        self.data.insert(sector, *bytes);
        Ok(())
    }
    fn flush(&mut self) -> Result<(), store::Error> {
        if self.fail {
            Err(store::Error::Device)
        } else {
            Ok(())
        }
    }
}
fn stage(authority: &mut Authority, disk: &mut Disk) -> Result<State, recovery::Error> {
    recovery::stage(
        authority,
        disk,
        COMPONENT,
        BUNDLE,
        ROOT,
        1,
        &mut vec![0; BUNDLE.len()],
    )
}
fn boot(authority: &mut Authority, disk: &mut Disk) -> Result<recovery::Boot, recovery::Error> {
    recovery::prepare_boot(authority, disk, ROOT, [1, 1], &mut vec![0; BUNDLE.len()])
}

#[test]
fn persisted_slot_and_actual_c_journal_survive_lost_stage_attempt_and_commit_replies() {
    let mut authority = Authority::new();
    let mut disk = Disk::default();
    authority.lose_mutation_reply = true;
    let staged = stage(&mut authority, &mut disk).unwrap();
    assert_eq!(staged.phase, Phase::Pending);
    assert_eq!(staged.committed(COMPONENT), Identity::INITIAL);
    let writes = disk.writes;
    assert_eq!(stage(&mut authority, &mut disk), Err(recovery::Error::Busy));
    assert_eq!(writes, disk.writes);
    let trial = boot(&mut authority, &mut disk).unwrap();
    let image = trial.selected(COMPONENT);
    assert_eq!(image.generation, 2);
    assert_eq!(trial.committed(COMPONENT), Identity::INITIAL);
    assert_eq!(authority.state().attempts, 1);
    assert_eq!(
        trial.commit(&mut authority).unwrap().committed(COMPONENT),
        image
    );
    let rebooted = boot(&mut authority, &mut disk).unwrap();
    assert_eq!(rebooted.selected(COMPONENT), image);
    assert_eq!(rebooted.committed(COMPONENT), image);
    assert!(rebooted.trial().is_none());
    disk.data.clear();
    assert_eq!(
        boot(&mut authority, &mut disk).unwrap_err(),
        recovery::Error::CommittedUnavailable(COMPONENT)
    );
    assert_eq!(authority.state().committed(COMPONENT), image);
}

#[test]
fn interruption_consumes_bounded_trials_then_durably_returns_to_committed() {
    let mut authority = Authority::new();
    let mut disk = Disk::default();
    stage(&mut authority, &mut disk).unwrap();
    for attempt in 1..=2 {
        assert!(boot(&mut authority, &mut disk).unwrap().trial().is_some());
        assert_eq!(authority.state().attempts, attempt);
    }
    let recovered = boot(&mut authority, &mut disk).unwrap();
    assert!(recovered.rolled_back());
    assert!(recovered.trial().is_none());
    assert_eq!(recovered.selected(COMPONENT), Identity::INITIAL);
    assert_eq!(authority.state().phase, Phase::Idle);
}

#[test]
fn live_monitor_trials_resolve_exact_abort_and_commit_without_advancing_failed_floor() {
    let mut authority = Authority::new();
    let mut disk = Disk::default();
    authority.lose_mutation_reply = true;
    let staged = stage(&mut authority, &mut disk).unwrap();
    let image = staged.pending.unwrap().1;
    let trial = recovery::begin_live_monitor(&mut authority, image).unwrap();
    assert_eq!(trial.committed(COMPONENT), Identity::INITIAL);
    let aborted = trial.abort(&mut authority).unwrap();
    assert_eq!(aborted.committed(COMPONENT), Identity::INITIAL);
    assert_eq!(
        aborted.last_change(),
        Some((
            bexos_secure_firmware::selection::Operation::Abort,
            COMPONENT,
            image
        ))
    );
    stage(&mut authority, &mut disk).unwrap();
    let trial = recovery::begin_live_monitor(&mut authority, image).unwrap();
    let committed = trial.commit(&mut authority).unwrap();
    assert_eq!(committed.committed(COMPONENT), image);
    assert_eq!(
        committed.last_change(),
        Some((
            bexos_secure_firmware::selection::Operation::Commit,
            COMPONENT,
            image
        ))
    );
    assert_eq!(
        recovery::begin_live_monitor(&mut authority, image).unwrap_err(),
        recovery::Error::Conflict
    );
}

#[test]
fn failed_slot_write_never_publishes_and_corrupt_pending_never_executes() {
    let mut authority = Authority::new();
    let mut disk = Disk {
        fail: true,
        ..Default::default()
    };
    assert!(matches!(
        stage(&mut authority, &mut disk),
        Err(recovery::Error::Storage(_))
    ));
    assert_eq!(authority.state().revision, 1);
    disk.fail = false;
    stage(&mut authority, &mut disk).unwrap();
    disk.data.clear();
    let boot = boot(&mut authority, &mut disk).unwrap();
    assert!(boot.rolled_back());
    assert_eq!(boot.selected(COMPONENT), Identity::INITIAL);
}

#[test]
fn unknown_commit_requires_authenticated_resolution_and_cannot_be_aborted() {
    let mut authority = Authority::new();
    let mut disk = Disk::default();
    stage(&mut authority, &mut disk).unwrap();
    let trial = boot(&mut authority, &mut disk).unwrap();
    let image = trial.selected(COMPONENT);
    let ticket = trial.commit_request().unwrap();
    authority.lose_mutation_reply = true;
    authority.lose_query_reply = true;
    assert_eq!(trial.commit(&mut authority), Err(recovery::Error::Journal));
    assert_eq!(
        boot(&mut authority, &mut disk).unwrap_err(),
        recovery::Error::Journal
    );
    authority.lose_query_reply = false;
    assert_eq!(
        recovery::commit_prepared(&mut authority, &ticket)
            .unwrap()
            .committed(COMPONENT),
        image
    );
    let resolved = boot(&mut authority, &mut disk).unwrap();
    assert_eq!(resolved.selected(COMPONENT), image);
    assert!(resolved.trial().is_none());
    assert_eq!(
        resolved.abort(&mut authority),
        Err(recovery::Error::InvalidTrial)
    );
}

#[test]
fn unpublished_mutations_have_a_bounded_exact_retry() {
    let mut authority = Authority::new();
    let mut disk = Disk::default();
    authority.drop_mutation = true;
    assert_eq!(
        stage(&mut authority, &mut disk),
        Err(recovery::Error::Journal)
    );
    assert_eq!(authority.exchanges, 5); // Initial query, two mutation/read pairs.
    assert_eq!(authority.state().revision, 1);
}

#[test]
fn arm_recovery_uses_real_authority_and_resolves_a_lost_commit_reply() {
    use bexos_secure_firmware::Architecture;
    let mut authority = Authority::new();
    authority
        .bytes
        .copy_from_slice(&include_bytes!(env!("ARM_SELECTION_RECORDS"))[..512]);
    let mut disk = Disk {
        architecture: Some(Architecture::Aarch64),
        ..Disk::default()
    };
    let bundle = include_bytes!(env!("ARM_TEST_FIRMWARE"));
    let mut scratch = vec![0; bundle.len()];
    let staged = recovery::stage(
        &mut authority,
        &mut disk,
        Component::Trusty,
        bundle,
        ROOT,
        1,
        &mut scratch,
    )
    .unwrap();
    assert_eq!(staged.architecture(), Architecture::Aarch64);
    assert_eq!(staged.committed(Component::Hypervisor), Identity::INITIAL);
    let trial =
        recovery::prepare_boot(&mut authority, &mut disk, ROOT, [1, 1], &mut scratch).unwrap();
    assert_eq!(trial.selected(Component::Trusty).generation, 2);
    authority.lose_mutation_reply = true;
    let committed = trial.commit(&mut authority).unwrap();
    assert_eq!(committed.committed(Component::Trusty).generation, 2);
    let reboot =
        recovery::prepare_boot(&mut authority, &mut disk, ROOT, [2, 1], &mut scratch).unwrap();
    assert!(reboot.trial().is_none());
    assert_eq!(reboot.selected(Component::Trusty).generation, 2);
    // A foreign owner cannot interpret the ARM protected selection as x86.
    disk.architecture = Some(Architecture::X86_64);
    assert!(matches!(
        recovery::prepare_boot(&mut authority, &mut disk, ROOT, [1, 1], &mut scratch),
        Err(recovery::Error::Journal)
    ));
}
