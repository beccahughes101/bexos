pub mod debug;
pub mod fidl;
pub mod handle;
pub mod ipc;
pub mod memory;
pub mod power;
pub mod routing;
pub mod scheduler;
pub mod system;
pub mod task;
pub mod tee;
pub mod time;
pub mod vmar;

use handle::{HandleRecord, HandleTable};
use ipc::{ChannelTable, MessageScratch};
use memory::{MappingTable, VmoTable};
use power::PowerState;
use scheduler::ProfileTable;
use system::{InterruptTable, ProcessTable, ResourceGroupTable, VmSpaceTable};
use task::{FutexTable, ThreadTable};
use time::ClockState;
use vmar::VmarTable;

use crate::cpu_features::{AsidAllocator, AsidSupport};
use crate::sched::Scheduler;

pub const RIGHT_TRANSFER: u32 = 0x0000_0001;
pub const RIGHT_READ: u32 = 0x0000_0002;
pub const RIGHT_WRITE: u32 = 0x0000_0004;
pub const RIGHT_EXECUTE: u32 = 0x0000_0008;
pub const RIGHT_MAP: u32 = 0x0000_0010;
pub const RIGHT_DUPLICATE: u32 = 0x0000_0020;
pub const RIGHT_SIGNAL: u32 = 0x0000_0040;
pub const RIGHT_MANAGE_TASK: u32 = 0x0000_0080;
pub const RIGHT_SET_POLICY: u32 = 0x0000_0100;
pub const RIGHT_ADMIN: u32 = 0x8000_0000;

pub const SIGNAL_READABLE: u32 = 0x0000_0001;
pub const SIGNAL_WRITABLE: u32 = 0x0000_0002;
pub const SIGNAL_PEER_CLOSED: u32 = 0x0000_0004;
pub const SIGNAL_SIGNALED: u32 = 0x0000_0008;
pub const SIGNAL_SUSPENDED: u32 = 0x0000_0010;
pub const SIGNAL_TERMINATED: u32 = 0x0000_0020;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelServiceStatus {
    Ok,
    InvalidHandle,
    AccessDenied,
    NoMemory,
    BufferTooSmall,
    PeerClosed,
    TimedOut,
    AlreadyExists,
    InvalidArgs,
    ResourceExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HardwareAccess {
    None,
    Isolated,
    Direct,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Handle {
    pub raw: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadMessageResult {
    pub status: KernelServiceStatus,
    pub bytes_len: usize,
    pub handles_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MapResult {
    pub status: KernelServiceStatus,
    pub mapped_vaddr: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitManyItem {
    pub handle: Handle,
    pub signals: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitManyResult {
    pub status: KernelServiceStatus,
    pub satisfied_index: u32,
    pub observed_signals: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WakeResult {
    pub status: KernelServiceStatus,
    pub woken_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CheckpointResult {
    pub status: KernelServiceStatus,
    pub serialized_bytes: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ControlPlane {
    pub handles: HandleTable,
    pub channels: ChannelTable,
    pub vmos: VmoTable,
    pub mappings: MappingTable,
    pub vmars: VmarTable,
    pub threads: ThreadTable,
    pub futexes: FutexTable,
    pub processes: ProcessTable,
    pub vm_spaces: VmSpaceTable,
    pub interrupts: InterruptTable,
    pub resource_groups: ResourceGroupTable,
    pub profiles: ProfileTable,
    pub clock: ClockState,
    pub power: PowerState,
    pub asids: AsidAllocator,
    pub scheduler: Scheduler,
    pub scratch: MessageScratch,
}

impl ControlPlane {
    pub const fn empty() -> Self {
        Self {
            handles: HandleTable::new(),
            channels: ChannelTable::new(),
            vmos: VmoTable::new(),
            mappings: MappingTable::new(),
            vmars: VmarTable::new(),
            threads: ThreadTable::new(),
            futexes: FutexTable::new(),
            processes: ProcessTable::new(),
            vm_spaces: VmSpaceTable::new(),
            interrupts: InterruptTable::new(),
            resource_groups: ResourceGroupTable::new(),
            profiles: ProfileTable::new(),
            clock: ClockState::new(),
            power: PowerState::new(),
            asids: AsidAllocator::disabled(),
            scheduler: Scheduler::empty(),
            scratch: MessageScratch::new(),
        }
    }

    pub fn new() -> Self {
        Self::with_cpu_count(1)
    }

    pub fn with_cpu_count(max_cpus: u32) -> Self {
        let mut control_plane = Self::empty();
        control_plane.resource_groups = ResourceGroupTable::with_defaults();
        control_plane.scheduler =
            Scheduler::with_cpu_count(max_cpus).expect("valid control-plane CPU count");
        for group in control_plane.resource_groups.iter() {
            let _ = control_plane.scheduler.ensure_resource_group_with_limits(
                group.id,
                group.parent_id,
                group.cpu_shares,
                group.max_cpu_utilization_permille,
                group.allow_realtime,
            );
        }
        control_plane
    }

    pub fn with_cpu_count_and_asids(max_cpus: u32, support: AsidSupport) -> Self {
        let mut control_plane = Self::with_cpu_count(max_cpus);
        control_plane.asids = AsidAllocator::new(support);
        control_plane
    }

    pub fn handle_record(&self, handle: Handle) -> Result<HandleRecord, KernelServiceStatus> {
        self.handles
            .get(handle.raw)
            .ok_or(KernelServiceStatus::InvalidHandle)
    }
}

impl Default for ControlPlane {
    fn default() -> Self {
        Self::new()
    }
}
