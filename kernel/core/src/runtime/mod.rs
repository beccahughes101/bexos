//! Executable runtime shared by the bare-metal kernel and backend tests.
//! Object IDs never cross the ABI: callers hold process-owned capabilities.
use crate::cpu_features::{AsidAllocator, AsidSupport};
use crate::kernel_services::{
    RIGHT_ADMIN as ADMIN, RIGHT_DUPLICATE as DUPLICATE, RIGHT_EXECUTE as EXECUTE, RIGHT_MAP as MAP,
    RIGHT_READ as READ, RIGHT_TRANSFER as TRANSFER, RIGHT_WRITE as WRITE,
};
use crate::sched::{
    BlockReason, DeadlineProfile, FairProfile, MAX_WAIT_MANY_ITEMS, Scheduler, SchedulerTask,
    SchedulingProfile,
};
use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use bexos_time_abi::SlewState;
use kernel_fidl::Status;
mod dma_snapshot;
pub mod handover;
pub mod incremental;
pub mod memory;
mod process_control;
mod reclamation;
mod restricted;
mod slots;
pub mod snapshot;
use incremental::{CHANNEL, HANDLE, META, PIN, PROCESS, PROFILE, SOCKET, THREAD, VMAR, VMO};
pub use memory::{AddressSpaceSwitch, Backend, Mapping, PacKeyMaterial, Vmar, Vmo, VmoBacking};
pub use memory::{DmaMapping, IommuDomain};

pub type Result<T> = core::result::Result<T, Status>;
pub const ALL_MEMORY: u32 = READ | WRITE | EXECUTE | MAP | TRANSFER | DUPLICATE;
pub const CHANNEL_RIGHTS: u32 = READ | WRITE | TRANSFER | DUPLICATE;
pub const IN_TRANSIT: usize = usize::MAX;
pub const MAX_PROCESSES: usize = 48;
pub const MAX_THREADS: usize = 256;
pub const DEFAULT_INTERRUPT_FLOOD_LIMIT_PER_SECOND: u32 = 100_000;

mod context;
pub use context::Context;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Object {
    Vmo(usize),
    Vmar(usize),
    Channel(usize, usize),
    Socket(usize, usize),
    Process(usize),
    Space(usize),
    Thread(usize),
    Profile(usize),
    ReplyToken(usize, usize, u64),
    IommuDomain(usize),
    Interrupt(usize),
}
#[derive(Clone, Copy, Debug)]
pub struct Capability {
    pub object: Object,
    pub rights: u32,
    pub owner: usize,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub bytes: Vec<u8>,
    pub handles: Vec<u64>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelPolicy {
    pub enable_priority_inheritance: bool,
    pub enable_timeslice_donation: bool,
}
impl ChannelPolicy {
    pub const fn disabled() -> Self {
        Self {
            enable_priority_inheritance: false,
            enable_timeslice_donation: false,
        }
    }
}
pub struct RuntimeCall {
    pub id: u64,
    pub caller_end: usize,
    pub caller_thread: usize,
    pub request: Message,
    pub reply: Option<Message>,
    pub serving: bool,
    pub server_thread: Option<usize>,
}
pub struct Channel {
    // Endpoint identity survives storage-slot reuse and migration.
    identity: u64,
    queues: [VecDeque<Message>; 2],
    calls: [VecDeque<RuntimeCall>; 2],
    refs: [usize; 2],
    next_call_id: u64,
    policy: ChannelPolicy,
}
pub struct Socket {
    queues: [VecDeque<Vec<u8>>; 2],
    refs: [usize; 2],
    read_closed: [bool; 2],
    write_closed: [bool; 2],
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SocketInfo {
    pub readable_bytes: u64,
    pub local_read_closed: bool,
    pub local_write_closed: bool,
    pub peer_read_closed: bool,
    pub peer_write_closed: bool,
}
pub struct Process {
    pub authority: u32,
    pub quarantined: bool,
    pub name: String,
    pub package: String,
    pub hardware: u32,
    pub resource_group_id: u32,
    pub realtime_scheduling: bool,
    pub root: u64,
    pub asid: u16,
    pub userspace_pac_key: PacKeyMaterial,
    pub context: Context,
    pub running: bool,
    pub exited: bool,
    pub root_vmar: usize,
    pub heap_vmar: usize,
    pub mappings: Vec<Mapping>,
    pub next_va: u64,
}
pub struct Thread {
    pub process: usize,
    pub context: Context,
    pub running: bool,
    pub exited: bool,
    pub blocked_futex: Option<u64>,
    pub blocked_wait_many: bool,
    pub exit_code: i32,
    pub restricted: Option<RestrictedBinding>,
}

#[derive(Clone, Copy, Debug)]
pub struct RestrictedBinding {
    pub state_vmo: usize,
    pub host_context: Context,
    pub guest_context: Context,
    pub host_readonly_thread_pointer: u64,
    pub guest_readonly_thread_pointer: u64,
    pub vector_entry: u64,
    pub vector_context: u64,
    pub active: bool,
    pub pending_kick: bool,
    pub transition_pending: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Interrupt {
    pub irq_number: u32,
    pub flags: u32,
    pub masked: bool,
    pub awaiting_ack: bool,
    pub window_started_ns: u64,
    pub events_in_window: u32,
    pub flood_limit_per_second: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptDelivery {
    Signaled,
    FloodLimited,
    Masked,
    NotBound,
}
pub struct Runtime<B: Backend> {
    pub handover: Option<handover::Handover>,
    dirty: Option<bexos_migration::dirty::DirtySet>,
    // Transient traversal state; pending ownership lives in zero-ref VMOs and
    // is serialized with them. Restore starts scanning from the beginning.
    defer_reclamation: bool,
    reclaim_cursor: usize,
    pub backend: B,
    pub processes: Vec<Process>,
    pub current: usize,
    pub threads: Vec<Thread>,
    pub current_thread: usize,
    pub vmos: Vec<Option<Vmo>>,
    pub vmars: Vec<Option<Vmar>>,
    pub handles: Vec<Option<Capability>>,
    pub channels: Vec<Channel>,
    pub sockets: Vec<Socket>,
    pub profiles: Vec<SchedulingProfile>,
    pub scheduler: Scheduler,
    pub pins: Vec<Option<(usize, usize)>>,
    pub iommu_domains: Vec<Option<IommuDomain>>,
    pub dma_mappings: Vec<Option<DmaMapping>>,
    pub interrupts: Vec<Option<Interrupt>>,
    asids: AsidAllocator,
    pub bootfs_pages: u64,
    pub reclaimed_pages: u64,
    pub zero_page: Option<u64>,
    pac_seed: [u64; 4],
    entropy: Option<random::Entropy>,
    realtime_slew: SlewState,
    time_page_vmo: Option<usize>,
}
impl<B: Backend> Runtime<B> {
    pub fn new(backend: B) -> Self {
        Self {
            handover: None,
            dirty: None,
            defer_reclamation: false,
            reclaim_cursor: 0,
            backend,
            processes: Vec::new(),
            current: 0,
            threads: Vec::new(),
            current_thread: 0,
            vmos: Vec::new(),
            vmars: Vec::new(),
            handles: Vec::new(),
            channels: Vec::new(),
            sockets: Vec::new(),
            profiles: Vec::new(),
            scheduler: Scheduler::new(),
            pins: Vec::new(),
            iommu_domains: Vec::new(),
            dma_mappings: Vec::new(),
            interrupts: Vec::new(),
            asids: AsidAllocator::disabled(),
            bootfs_pages: 0,
            reclaimed_pages: 0,
            zero_page: None,
            pac_seed: [
                0x1a2b_3c4d_5e6f_7788,
                0x8877_6f5e_4d3c_2b1a,
                0x3243_f6a8_885a_308d,
                0x1319_8a2e_0370_7344,
            ],
            entropy: None,
            realtime_slew: SlewState::new(),
            time_page_vmo: None,
        }
    }
    pub fn with_asid_support(backend: B, support: AsidSupport) -> Self {
        let mut runtime = Self::new(backend);
        runtime.asids = AsidAllocator::new(support);
        runtime
    }

    pub fn with_cpu_count(backend: B, cpu_count: u32, support: AsidSupport) -> Self {
        let mut runtime = Self::with_asid_support(backend, support);
        runtime.scheduler = Scheduler::with_cpu_count(cpu_count).unwrap_or_default();
        runtime
    }
    pub fn capability(&self, h: u64, rights: u32) -> Result<Capability> {
        let cap = self
            .handles
            .get(h.checked_sub(1).ok_or(Status::ErrInvalidHandle)? as usize)
            .and_then(|c| *c)
            .ok_or(Status::ErrInvalidHandle)?;
        if cap.owner != self.current || cap.rights & rights != rights {
            return Err(Status::ErrAccessDenied);
        }
        if self.processes[self.current].quarantined {
            if let Some(h) = &self.handover {
                let shared = self
                    .handles
                    .iter()
                    .flatten()
                    .any(|other| other.owner == h.source && other.object == cap.object);
                let mapped = matches!(cap.object, Object::Vmo(id) if self.processes[h.source].mappings.iter().any(|m| m.vmo == id));
                if shared || mapped {
                    return Err(Status::ErrAccessDenied);
                }
            }
        }
        Ok(cap)
    }

    pub fn set_boot_entropy_seed(&mut self, seed: Option<[u64; 4]>) {
        if let Some(seed) = seed {
            if seed.iter().any(|word| *word != 0) {
                self.pac_seed = seed;
                self.entropy = Some(random::Entropy::from_boot(seed));
                self.changed(META, 0);
            }
        }
    }

    fn process_pac_key(&self, process_id: usize, asid: u16) -> PacKeyMaterial {
        let mut state = self.pac_seed[0]
            ^ self.pac_seed[1].rotate_left(17)
            ^ self.pac_seed[2].rotate_left(31)
            ^ self.pac_seed[3].rotate_left(47)
            ^ ((process_id as u64) << 32)
            ^ asid as u64
            ^ 0xbe0_5000_0000_0001;
        PacKeyMaterial {
            lo: splitmix64(&mut state),
            hi: splitmix64(&mut state),
        }
    }
    pub fn grant(&mut self, owner: usize, object: Object, rights: u32) -> u64 {
        match object {
            Object::Vmo(id) => self.changed(VMO, id),
            Object::Vmar(id) => self.changed(VMAR, id),
            Object::Channel(id, _) => self.changed(CHANNEL, id),
            Object::Socket(id, _) => self.changed(SOCKET, id),
            Object::Profile(id) => self.changed(PROFILE, id),
            Object::ReplyToken(id, _, _) => self.changed(CHANNEL, id),
            Object::IommuDomain(_) => {}
            Object::Interrupt(id) => self.changed(incremental::INTERRUPT, id),
            _ => {}
        }
        match object {
            Object::Vmo(id) => self.vmos[id].as_mut().unwrap().refs += 1,
            Object::Vmar(id) => self.vmars[id].as_mut().unwrap().refs += 1,
            Object::Channel(id, end) => self.channels[id].refs[end] += 1,
            Object::Socket(id, end) => self.sockets[id].refs[end] += 1,
            Object::IommuDomain(id) => {
                self.iommu_domains[id].as_mut().unwrap().refs += 1;
                self.changed(incremental::IOMMU_DOMAIN, id);
            }
            _ => {}
        }
        let index = self
            .handles
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.handles.len());
        let cap = Some(Capability {
            object,
            rights,
            owner,
        });
        if index == self.handles.len() {
            self.handles.push(cap);
        } else {
            self.handles[index] = cap;
        }
        self.changed(HANDLE, index);
        index as u64 + 1
    }
    pub fn duplicate(&mut self, handle: u64, rights: u32) -> Result<u64> {
        let cap = self.capability(handle, DUPLICATE)?;
        if rights & !cap.rights != 0 {
            return Err(Status::ErrAccessDenied);
        }
        Ok(self.grant(self.current, cap.object, rights))
    }
    pub fn close(&mut self, handle: u64) -> Result<()> {
        let cap = self.capability(handle, 0)?;
        self.handles[handle as usize - 1] = None;
        self.changed(HANDLE, handle as usize - 1);
        self.release_object(cap.object);
        Ok(())
    }
    fn release_object(&mut self, object: Object) {
        match object {
            Object::Vmo(id) => self.release_vmo(id),
            Object::Vmar(id) => self.release_vmar(id),
            Object::IommuDomain(id) => self.release_iommu_domain(id),
            Object::Interrupt(id) => {
                if let Some(slot) = self.interrupts.get_mut(id) {
                    *slot = None;
                }
                self.changed(incremental::INTERRUPT, id);
            }
            Object::Channel(id, end) => {
                self.changed(CHANNEL, id);
                self.channels[id].refs[end] -= 1;
                if self.channels[id].refs[end] == 0 {
                    let messages = core::mem::take(&mut self.channels[id].queues[end]);
                    for msg in messages {
                        for h in msg.handles {
                            if let Some(c) = self.handles[h as usize - 1].take() {
                                self.changed(HANDLE, h as usize - 1);
                                self.release_object(c.object);
                            }
                        }
                    }
                }
            }
            Object::Socket(id, end) => {
                self.changed(SOCKET, id);
                self.sockets[id].refs[end] -= 1;
                if self.sockets[id].refs[end] == 0 {
                    self.sockets[id].read_closed[end] = true;
                    self.sockets[id].write_closed[end] = true;
                }
            }
            Object::Profile(id) => self.changed(PROFILE, id),
            Object::ReplyToken(id, _, _) => self.changed(CHANNEL, id),
            _ => {}
        }
    }

    pub fn create_scheduling_profile(&mut self, profile: SchedulingProfile) -> Result<u64> {
        let profile = self.validate_profile_request(profile)?;
        let id = self.profiles.len();
        self.profiles.push(profile);
        self.changed(PROFILE, id);
        Ok(self.grant(
            self.current,
            Object::Profile(id),
            ADMIN | TRANSFER | DUPLICATE,
        ))
    }

    fn validate_profile_request(&self, profile: SchedulingProfile) -> Result<SchedulingProfile> {
        match profile {
            SchedulingProfile::Fair(FairProfile { priority, weight }) => {
                if priority == 0 || weight == 0 {
                    return Err(Status::ErrInvalidArgs);
                }
                if priority > 127 && !self.processes[self.current].realtime_scheduling {
                    return Err(Status::ErrAccessDenied);
                }
                Ok(profile)
            }
            SchedulingProfile::Deadline(deadline) => {
                if !deadline.valid() {
                    return Err(Status::ErrInvalidArgs);
                }
                if !self.processes[self.current].realtime_scheduling {
                    return Err(Status::ErrAccessDenied);
                }
                Ok(SchedulingProfile::Deadline(deadline))
            }
        }
    }
    pub fn create_channel(&mut self) -> (u64, u64) {
        let id = self.insert_channel();
        (
            self.grant(self.current, Object::Channel(id, 0), CHANNEL_RIGHTS),
            self.grant(self.current, Object::Channel(id, 1), CHANNEL_RIGHTS),
        )
    }
    pub fn channel_identity(&self, handle: u64) -> Result<(u64, u64)> {
        let Object::Channel(id, end) = self.capability(handle, 0)?.object else {
            return Err(Status::ErrInvalidArgs);
        };
        let base = self.channels[id].identity;
        Ok((base + end as u64, base + (1 - end) as u64))
    }
    pub fn create_socket_pair(&mut self) -> (u64, u64) {
        let id = self.sockets.len();
        self.sockets.push(Socket {
            queues: [VecDeque::new(), VecDeque::new()],
            refs: [0, 0],
            read_closed: [false, false],
            write_closed: [false, false],
        });
        (
            self.grant(self.current, Object::Socket(id, 0), CHANNEL_RIGHTS),
            self.grant(self.current, Object::Socket(id, 1), CHANNEL_RIGHTS),
        )
    }
    pub fn write_message(&mut self, h: u64, bytes: &[u8], handles: &[u64]) -> Result<()> {
        let Object::Channel(id, end) = self.capability(h, WRITE)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if bytes.len() > 65536 || handles.len() > 64 {
            return Err(Status::ErrBufferTooSmall);
        }
        if self.channels[id].refs[1 - end] == 0 {
            return Err(Status::ErrPeerClosed);
        }
        if self.channels[id].queues[1 - end].len() >= 64 {
            return Err(Status::ErrNoMemory);
        }
        for (i, handle) in handles.iter().enumerate() {
            self.capability(*handle, TRANSFER)?;
            if handles[..i].contains(handle) || *handle == h {
                return Err(Status::ErrInvalidArgs);
            }
        }
        let msg = Message {
            bytes: bytes.to_vec(),
            handles: handles.to_vec(),
        };
        for handle in handles {
            self.handles[*handle as usize - 1].as_mut().unwrap().owner = IN_TRANSIT;
            self.changed(HANDLE, *handle as usize - 1);
        }
        self.channels[id].queues[1 - end].push_back(msg);
        self.changed(CHANNEL, id);
        self.wake_waiters_for_object(
            Object::Channel(id, 1 - end),
            crate::kernel_services::SIGNAL_READABLE,
        );
        Ok(())
    }

    fn prepare_transfer_message(
        &mut self,
        own_handle: u64,
        bytes: &[u8],
        handles: &[u64],
    ) -> Result<Message> {
        if bytes.len() > 65536 || handles.len() > 64 {
            return Err(Status::ErrBufferTooSmall);
        }
        for (i, handle) in handles.iter().enumerate() {
            self.capability(*handle, TRANSFER)?;
            if handles[..i].contains(handle) || *handle == own_handle {
                return Err(Status::ErrInvalidArgs);
            }
        }
        for handle in handles {
            self.handles[*handle as usize - 1].as_mut().unwrap().owner = IN_TRANSIT;
            self.changed(HANDLE, *handle as usize - 1);
        }
        Ok(Message {
            bytes: bytes.to_vec(),
            handles: handles.to_vec(),
        })
    }

    fn restore_message_handles(&mut self, message: &Message, owner: usize) {
        for handle in &message.handles {
            if let Some(capability) = self.handles[*handle as usize - 1].as_mut() {
                capability.owner = owner;
                self.changed(HANDLE, *handle as usize - 1);
            }
        }
    }

    fn deliver_runtime_message(
        &mut self,
        message: Message,
        max_bytes: usize,
        max_handles: usize,
    ) -> Result<Message> {
        if message.bytes.len() > max_bytes || message.handles.len() > max_handles {
            return Err(Status::ErrBufferTooSmall);
        }
        for handle in &message.handles {
            self.handles[*handle as usize - 1].as_mut().unwrap().owner = self.current;
            self.changed(HANDLE, *handle as usize - 1);
        }
        Ok(message)
    }

    fn has_pending_call_for_current_thread(&self, channel_id: usize, caller_end: usize) -> bool {
        let caller_thread = self.current_thread;
        self.channels[channel_id].calls[1 - caller_end]
            .iter()
            .any(|call| {
                call.caller_end == caller_end
                    && call.caller_thread == caller_thread
                    && call.reply.is_none()
            })
    }

    fn has_completed_call_reply_for_current_thread(
        &self,
        channel_id: usize,
        caller_end: usize,
    ) -> bool {
        let caller_thread = self.current_thread;
        self.channels[channel_id].calls[1 - caller_end]
            .iter()
            .any(|call| {
                call.caller_end == caller_end
                    && call.caller_thread == caller_thread
                    && call.reply.is_some()
            })
    }

    fn take_completed_call_reply(
        &mut self,
        channel_id: usize,
        caller_end: usize,
    ) -> Option<Message> {
        let caller_thread = self.current_thread;
        let queue = &mut self.channels[channel_id].calls[1 - caller_end];
        let index = queue.iter().position(|call| {
            call.caller_end == caller_end
                && call.caller_thread == caller_thread
                && call.reply.is_some()
        })?;
        queue.remove(index)?.reply
    }

    pub fn set_channel_policy(
        &mut self,
        h: u64,
        enable_priority_inheritance: bool,
        enable_timeslice_donation: bool,
    ) -> Result<()> {
        let Object::Channel(id, _) = self.capability(h, 0)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        self.channels[id].policy = ChannelPolicy {
            enable_priority_inheritance,
            enable_timeslice_donation,
        };
        self.changed(CHANNEL, id);
        Ok(())
    }

    pub fn call(
        &mut self,
        h: u64,
        bytes: &[u8],
        handles: &[u64],
        deadline_nanos: i64,
        max_reply_bytes: usize,
        max_reply_handles: usize,
    ) -> Result<Message> {
        let Object::Channel(id, end) = self.capability(h, READ | WRITE)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if let Some(reply) = self.take_completed_call_reply(id, end) {
            return self.deliver_runtime_message(reply, max_reply_bytes, max_reply_handles);
        }
        if self.has_pending_call_for_current_thread(id, end) {
            return Err(Status::ErrTimedOut);
        }
        if runtime_deadline_expired(self.backend.monotonic_ns().unwrap_or(0), deadline_nanos) {
            return Err(Status::ErrTimedOut);
        }
        let request = self.prepare_transfer_message(h, bytes, handles)?;
        let channel = &mut self.channels[id];
        if channel.refs[1 - end] == 0 {
            self.restore_message_handles(&request, self.current);
            return Err(Status::ErrPeerClosed);
        }
        let call_id = channel.next_call_id;
        channel.next_call_id = channel.next_call_id.saturating_add(1);
        channel.calls[1 - end].push_back(RuntimeCall {
            id: call_id,
            caller_end: end,
            caller_thread: self.current_thread,
            request,
            reply: None,
            serving: false,
            server_thread: None,
        });
        self.changed(CHANNEL, id);
        let _ = self.scheduler.block_current(BlockReason::HandleSignals {
            handle: h,
            signals: crate::kernel_services::SIGNAL_READABLE,
        });
        Err(Status::ErrTimedOut)
    }

    pub fn read_call(
        &mut self,
        h: u64,
        deadline_nanos: i64,
        max_bytes: usize,
        max_handles: usize,
    ) -> Result<(Message, u64)> {
        let Object::Channel(id, end) = self.capability(h, READ)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if runtime_deadline_expired(self.backend.monotonic_ns().unwrap_or(0), deadline_nanos)
            && self.channels[id].calls[end]
                .iter()
                .all(|call| call.serving || call.reply.is_some())
        {
            return Err(Status::ErrTimedOut);
        }
        let Some(index) = self.channels[id].calls[end]
            .iter()
            .position(|call| !call.serving && call.reply.is_none())
        else {
            return Err(Status::ErrTimedOut);
        };
        let call_id = self.channels[id].calls[end][index].id;
        let request = self.channels[id].calls[end][index].request.clone();
        let delivered = self.deliver_runtime_message(request, max_bytes, max_handles)?;
        self.channels[id].calls[end][index].serving = true;
        self.channels[id].calls[end][index].server_thread = Some(self.current_thread);
        let token = self.grant(self.current, Object::ReplyToken(id, end, call_id), WRITE);
        if self.channels[id].policy.enable_priority_inheritance
            || self.channels[id].policy.enable_timeslice_donation
        {
            let caller_thread = self.channels[id].calls[end][index].caller_thread;
            if let Some(caller) = self.scheduler.task(caller_thread as u64 + 1) {
                let _ = self
                    .scheduler
                    .donate_priority(self.current_thread as u64 + 1, caller.effective_priority);
            }
        }
        self.changed(CHANNEL, id);
        Ok((delivered, token))
    }

    pub fn reply_call(&mut self, token: u64, bytes: &[u8], handles: &[u64]) -> Result<()> {
        let Object::ReplyToken(id, end, call_id) = self.capability(token, WRITE)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let reply = self.prepare_transfer_message(token, bytes, handles)?;
        self.handles[token as usize - 1] = None;
        self.changed(HANDLE, token as usize - 1);
        let Some(call) = self.channels[id].calls[end]
            .iter_mut()
            .find(|call| call.id == call_id && call.serving)
        else {
            self.restore_message_handles(&reply, self.current);
            return Err(Status::ErrInvalidHandle);
        };
        call.reply = Some(reply);
        let caller_thread = call.caller_thread;
        let server_thread = call.server_thread;
        if let Some(server_thread) = server_thread {
            let _ = self
                .scheduler
                .restore_base_priority(server_thread as u64 + 1);
        }
        let _ = self.scheduler.wake_task(caller_thread as u64 + 1);
        self.changed(CHANNEL, id);
        Ok(())
    }
    pub fn write_socket(&mut self, h: u64, bytes: &[u8]) -> Result<usize> {
        let Object::Socket(id, end) = self.capability(h, WRITE)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if bytes.len() > 65536 {
            return Err(Status::ErrBufferTooSmall);
        }
        let socket = &mut self.sockets[id];
        if socket.write_closed[end] || socket.read_closed[1 - end] || socket.refs[1 - end] == 0 {
            return Err(Status::ErrPeerClosed);
        }
        if socket.queues[1 - end].len() >= 64 {
            return Err(Status::ErrNoMemory);
        }
        socket.queues[1 - end].push_back(bytes.to_vec());
        self.changed(SOCKET, id);
        Ok(bytes.len())
    }
    pub fn read_socket(&mut self, h: u64, max_bytes: usize) -> Result<Vec<u8>> {
        let Object::Socket(id, end) = self.capability(h, READ)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if max_bytes > 65536 {
            return Err(Status::ErrBufferTooSmall);
        }
        let socket = &mut self.sockets[id];
        if socket.read_closed[end] {
            return Err(Status::ErrPeerClosed);
        }
        let Some(mut bytes) = socket.queues[end].pop_front() else {
            return Err(
                if socket.write_closed[1 - end] || socket.refs[1 - end] == 0 {
                    Status::ErrPeerClosed
                } else {
                    Status::ErrTimedOut
                },
            );
        };
        if bytes.len() > max_bytes {
            let rest = bytes.split_off(max_bytes);
            socket.queues[end].push_front(rest);
        }
        self.changed(SOCKET, id);
        Ok(bytes)
    }
    pub fn shutdown_socket(&mut self, h: u64, read: bool, write: bool) -> Result<()> {
        let Object::Socket(id, end) = self.capability(h, 0)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if read && !self.capability(h, READ).is_ok() {
            return Err(Status::ErrAccessDenied);
        }
        if write && !self.capability(h, WRITE).is_ok() {
            return Err(Status::ErrAccessDenied);
        }
        let socket = &mut self.sockets[id];
        if read {
            socket.read_closed[end] = true;
            socket.queues[end].clear();
        }
        if write {
            socket.write_closed[end] = true;
        }
        self.changed(SOCKET, id);
        Ok(())
    }
    pub fn socket_info(&self, h: u64) -> Result<SocketInfo> {
        let cap = self.capability(h, 0)?;
        let Object::Socket(id, end) = cap.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let socket = &self.sockets[id];
        Ok(SocketInfo {
            readable_bytes: socket.queues[end]
                .iter()
                .map(|entry| entry.len() as u64)
                .sum(),
            local_read_closed: socket.read_closed[end],
            local_write_closed: socket.write_closed[end],
            peer_read_closed: socket.read_closed[1 - end] || socket.refs[1 - end] == 0,
            peer_write_closed: socket.write_closed[1 - end] || socket.refs[1 - end] == 0,
        })
    }

    pub fn bind_interrupt(&mut self, irq_number: u32, flags: u32) -> Result<u64> {
        if self.processes[self.current].hardware != 2
            || self.processes[self.current].quarantined
        {
            return Err(Status::ErrAccessDenied);
        }
        if self.interrupts.iter().flatten().any(|interrupt| {
            interrupt.irq_number == irq_number && (interrupt.flags & 1 == 0 || flags & 1 == 0)
        }) {
            return Err(Status::ErrAlreadyExists);
        }
        let id = self
            .interrupts
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.interrupts.len());
        let interrupt = Some(Interrupt {
            irq_number,
            flags,
            masked: false,
            awaiting_ack: false,
            window_started_ns: 0,
            events_in_window: 0,
            flood_limit_per_second: DEFAULT_INTERRUPT_FLOOD_LIMIT_PER_SECOND,
        });
        if id == self.interrupts.len() {
            self.interrupts.push(interrupt);
        } else {
            self.interrupts[id] = interrupt;
        }
        self.changed(incremental::INTERRUPT, id);
        Ok(self.grant(
            self.current,
            Object::Interrupt(id),
            READ | WRITE | ADMIN | TRANSFER | DUPLICATE,
        ))
    }

    pub fn acknowledge_interrupt(&mut self, handle: u64) -> Result<(u32, bool)> {
        let Object::Interrupt(id) = self.capability(handle, WRITE)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let interrupt = self
            .interrupts
            .get_mut(id)
            .and_then(Option::as_mut)
            .ok_or(Status::ErrInvalidHandle)?;
        if !interrupt.awaiting_ack {
            return Err(Status::ErrInvalidArgs);
        }
        interrupt.awaiting_ack = false;
        interrupt.masked = false;
        let irq_number = interrupt.irq_number;
        self.changed(incremental::INTERRUPT, id);
        let hardware_masked = self.interrupts.iter().flatten().any(|interrupt| {
            interrupt.irq_number == irq_number
                && (interrupt.masked || interrupt.awaiting_ack)
        });
        Ok((irq_number, hardware_masked))
    }

    pub fn mask_interrupt(&mut self, handle: u64, masked: bool) -> Result<(u32, bool)> {
        let Object::Interrupt(id) = self.capability(handle, WRITE)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let interrupt = self
            .interrupts
            .get_mut(id)
            .and_then(Option::as_mut)
            .ok_or(Status::ErrInvalidHandle)?;
        if !masked && interrupt.awaiting_ack {
            return Err(Status::ErrInvalidArgs);
        }
        interrupt.masked = masked;
        let irq_number = interrupt.irq_number;
        self.changed(incremental::INTERRUPT, id);
        let hardware_masked = self.interrupts.iter().flatten().any(|interrupt| {
            interrupt.irq_number == irq_number
                && (interrupt.masked || interrupt.awaiting_ack)
        });
        Ok((irq_number, hardware_masked))
    }

    pub fn deliver_interrupt(&mut self, irq_number: u32, now_ns: u64) -> InterruptDelivery {
        let mut matched = false;
        let mut signaled = false;
        let mut flooded = false;
        let mut wake = Vec::new();
        for (id, slot) in self.interrupts.iter_mut().enumerate() {
            let Some(interrupt) = slot
                .as_mut()
                .filter(|interrupt| interrupt.irq_number == irq_number)
            else {
                continue;
            };
            matched = true;
            if interrupt.masked || interrupt.awaiting_ack {
                continue;
            }
            if now_ns.saturating_sub(interrupt.window_started_ns) >= 1_000_000_000 {
                interrupt.window_started_ns = now_ns;
                interrupt.events_in_window = 0;
            }
            interrupt.events_in_window = interrupt.events_in_window.saturating_add(1);
            let this_flooded =
                interrupt.events_in_window > interrupt.flood_limit_per_second;
            interrupt.masked = true;
            interrupt.awaiting_ack = !this_flooded;
            flooded |= this_flooded;
            signaled |= !this_flooded;
            wake.push(id);
        }
        for id in wake {
            self.changed(incremental::INTERRUPT, id);
            self.wake_waiters_for_object(
                Object::Interrupt(id),
                crate::kernel_services::SIGNAL_READABLE,
            );
        }
        if flooded {
            InterruptDelivery::FloodLimited
        } else if signaled {
            InterruptDelivery::Signaled
        } else if matched {
            InterruptDelivery::Masked
        } else {
            InterruptDelivery::NotBound
        }
    }

    pub fn object_signals(&self, h: u64) -> Result<u32> {
        let cap = self.capability(h, 0)?;
        let signals = match cap.object {
            Object::Channel(id, end) => {
                let channel = &self.channels[id];
                let mut signals = 0;
                if !channel.queues[end].is_empty()
                    || channel.calls[end]
                        .iter()
                        .any(|call| !call.serving && call.reply.is_none())
                    || self.has_completed_call_reply_for_current_thread(id, end)
                {
                    signals |= crate::kernel_services::SIGNAL_READABLE;
                }
                if channel.refs[1 - end] > 0 {
                    signals |= crate::kernel_services::SIGNAL_WRITABLE;
                } else {
                    signals |= crate::kernel_services::SIGNAL_PEER_CLOSED;
                }
                signals
            }
            Object::Socket(id, end) => {
                let socket = &self.sockets[id];
                let mut signals = 0;
                if !socket.queues[end].is_empty() {
                    signals |= crate::kernel_services::SIGNAL_READABLE;
                }
                if !socket.write_closed[end]
                    && !socket.read_closed[1 - end]
                    && socket.refs[1 - end] > 0
                {
                    signals |= crate::kernel_services::SIGNAL_WRITABLE;
                }
                if socket.refs[1 - end] == 0 || socket.write_closed[1 - end] {
                    signals |= crate::kernel_services::SIGNAL_PEER_CLOSED;
                }
                signals
            }
            Object::Thread(id) => {
                if self.threads[id].exited {
                    crate::kernel_services::SIGNAL_TERMINATED
                } else {
                    0
                }
            }
            Object::Process(id) => {
                if self.processes[id].exited {
                    crate::kernel_services::SIGNAL_TERMINATED
                } else {
                    0
                }
            }
            Object::Vmo(_)
            | Object::Vmar(_)
            | Object::Space(_)
            | Object::Profile(_)
            | Object::IommuDomain(_)
            | Object::ReplyToken(_, _, _) => crate::kernel_services::SIGNAL_WRITABLE,
            Object::Interrupt(id) => self
                .interrupts
                .get(id)
                .and_then(|interrupt| *interrupt)
                .filter(|interrupt| interrupt.awaiting_ack || interrupt.masked)
                .map_or(0, |_| crate::kernel_services::SIGNAL_READABLE),
        };
        Ok(signals)
    }
    pub fn read_message(
        &mut self,
        h: u64,
        max_bytes: usize,
        max_handles: usize,
    ) -> Result<Message> {
        let Object::Channel(id, end) = self.capability(h, READ)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let channel = &mut self.channels[id];
        let Some(front) = channel.queues[end].front() else {
            return Err(if channel.refs[1 - end] == 0 {
                Status::ErrPeerClosed
            } else {
                Status::ErrTimedOut
            });
        };
        if front.bytes.len() > max_bytes || front.handles.len() > max_handles {
            return Err(Status::ErrBufferTooSmall);
        }
        let msg = channel.queues[end].pop_front().unwrap();
        self.changed(CHANNEL, id);
        self.wake_waiters_for_object(
            Object::Channel(id, 1 - end),
            crate::kernel_services::SIGNAL_WRITABLE,
        );
        for handle in &msg.handles {
            self.handles[*handle as usize - 1].as_mut().unwrap().owner = self.current;
            self.changed(HANDLE, *handle as usize - 1);
        }
        Ok(msg)
    }
    pub fn create_process(
        &mut self,
        name: &str,
        package: &str,
        hardware: u32,
    ) -> Result<(u64, u64)> {
        self.create_process_with_policy(name, package, hardware, 1, false)
    }

    pub fn create_process_with_policy(
        &mut self,
        name: &str,
        package: &str,
        hardware: u32,
        resource_group_id: u32,
        realtime_scheduling: bool,
    ) -> Result<(u64, u64)> {
        if !self.processes.is_empty() && !self.has_authority(handover::AUTH_APP_MANAGER) {
            return Err(Status::ErrAccessDenied);
        }
        if name.is_empty()
            || name.len() > 64
            || package.len() > 96
            || hardware > 2
            || resource_group_id == 0
            || self.processes.len() >= MAX_PROCESSES
        {
            return Err(Status::ErrInvalidArgs);
        }
        let root = self.backend.new_space()?;
        let asid = self.asids.allocate().ok_or(Status::ErrNoMemory)?;
        let id = self.processes.len();
        let root_vmar = self.insert_vmar(
            id,
            None,
            bexos_boot::USER_START,
            bexos_boot::USER_END - bexos_boot::USER_START,
            READ | WRITE | EXECUTE | MAP | ADMIN,
        )?;
        self.processes.push(Process {
            authority: if id == 0 {
                handover::AUTH_APP_MANAGER
            } else {
                0
            },
            quarantined: false,
            name: name.to_string(),
            package: package.to_string(),
            hardware,
            resource_group_id,
            realtime_scheduling,
            root,
            asid,
            userspace_pac_key: self.process_pac_key(id, asid),
            context: Context::zero(),
            running: false,
            exited: false,
            root_vmar,
            heap_vmar: root_vmar,
            mappings: Vec::new(),
            next_va: bexos_boot::USER_IMAGE_BASE + 0x3000_0000,
        });
        let heap_vmar = self.insert_vmar(
            id,
            Some(root_vmar),
            bexos_boot::USER_HEAP_VMAR_BASE,
            bexos_boot::USER_HEAP_VMAR_SIZE,
            READ | WRITE | MAP | ADMIN,
        )?;
        self.processes[id].heap_vmar = heap_vmar;
        let _ = self.grant(id, Object::Vmar(heap_vmar), READ | WRITE | MAP | ADMIN);
        self.changed(PROCESS, id);
        Ok((
            self.grant(self.current, Object::Process(id), ADMIN),
            self.grant(self.current, Object::Space(id), ADMIN | MAP),
        ))
    }
    pub fn start(
        &mut self,
        process: u64,
        space: u64,
        entry: u64,
        stack: u64,
        arg: u64,
    ) -> Result<u64> {
        self.start_with_thread_pointer(process, space, entry, stack, 0, arg)
    }

    pub fn start_with_thread_pointer(
        &mut self,
        process: u64,
        space: u64,
        entry: u64,
        stack: u64,
        thread_pointer: u64,
        arg: u64,
    ) -> Result<u64> {
        let Object::Process(id) = self.capability(process, ADMIN)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if self.capability(space, MAP)?.object != Object::Space(id) {
            return Err(Status::ErrInvalidArgs);
        }
        if self.processes[id].running
            || self.processes[id].exited
            || !self.valid_range(id, entry, 4, EXECUTE)
            || stack % 16 != 0
            || stack < 16
            || !self.valid_range(id, stack - 16, 16, WRITE)
        {
            return Err(Status::ErrInvalidArgs);
        }
        let context =
            Context::user(entry, stack, arg, thread_pointer).ok_or(Status::ErrInvalidArgs)?;
        if arg != 0 {
            let cap = self.capability(arg, TRANSFER)?;
            if !matches!(cap.object, Object::Channel(..)) {
                return Err(Status::ErrInvalidHandle);
            }
            self.handles[arg as usize - 1].as_mut().unwrap().owner = id;
            self.changed(HANDLE, arg as usize - 1);
        }
        let thread = self.add_thread_to_process(id, context)?;
        let p = &mut self.processes[id];
        p.context = context;
        p.running = true;
        self.changed(PROCESS, id);
        Ok(self.grant(self.current, Object::Thread(thread), ADMIN))
    }

    pub fn terminate_process(&mut self, process: u64, exit_code: i32) -> Result<()> {
        let Object::Process(id) = self.capability(process, ADMIN)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if self.processes[id].exited {
            return Ok(());
        }
        self.processes[id].running = false;
        self.processes[id].exited = true;
        self.changed(PROCESS, id);
        for thread_id in 0..self.threads.len() {
            if self.threads[thread_id].process == id && !self.threads[thread_id].exited {
                let restricted_vmo = {
                    let thread = &mut self.threads[thread_id];
                    thread.running = false;
                    thread.exited = true;
                    thread.blocked_futex = None;
                    thread.blocked_wait_many = false;
                    thread.exit_code = exit_code;
                    thread.restricted.take().map(|binding| binding.state_vmo)
                };
                if let Some(vmo) = restricted_vmo {
                    self.release_vmo(vmo);
                }
                let _ = self.scheduler.exit_task(thread_id as u64 + 1, exit_code);
                self.changed(incremental::THREAD, thread_id);
            }
        }
        let owned_handles = self
            .handles
            .iter()
            .enumerate()
            .filter_map(|(index, cap)| {
                cap.filter(|capability| capability.owner == id)
                    .map(|_| index)
            })
            .collect::<Vec<_>>();
        for handle in owned_handles {
            if let Some(cap) = self.handles[handle].take() {
                self.changed(HANDLE, handle);
                self.release_object(cap.object);
            }
        }
        self.wake_waiters_for_object(
            Object::Process(id),
            crate::kernel_services::SIGNAL_TERMINATED,
        );
        Ok(())
    }

    pub fn create_thread_current(&mut self, entry: u64, stack: u64, arg: u64) -> Result<u64> {
        self.create_thread_current_with_thread_pointer(entry, stack, 0, arg)
    }

    pub fn create_thread_current_with_thread_pointer(
        &mut self,
        entry: u64,
        stack: u64,
        thread_pointer: u64,
        arg: u64,
    ) -> Result<u64> {
        let id = self.current;
        if !self.processes[id].running
            || self.processes[id].exited
            || !self.valid_range(id, entry, 4, EXECUTE)
            || stack % 16 != 0
            || stack < 16
            || !self.valid_range(id, stack - 16, 16, WRITE)
        {
            return Err(Status::ErrInvalidArgs);
        }
        let context =
            Context::user(entry, stack, arg, thread_pointer).ok_or(Status::ErrInvalidArgs)?;
        let thread = self.add_thread_to_process(id, context)?;
        Ok(self.grant(self.current, Object::Thread(thread), ADMIN))
    }

    pub fn set_thread_profile(&mut self, thread_handle: u64, profile_handle: u64) -> Result<()> {
        let Object::Thread(thread_id) = self.capability(thread_handle, ADMIN)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let Object::Profile(profile_id) = self.capability(profile_handle, ADMIN)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let profile = *self
            .profiles
            .get(profile_id)
            .ok_or(Status::ErrInvalidHandle)?;
        let thread = self
            .threads
            .get(thread_id)
            .ok_or(Status::ErrInvalidHandle)?;
        let process = self
            .processes
            .get(thread.process)
            .ok_or(Status::ErrInvalidHandle)?;
        match profile {
            SchedulingProfile::Fair(fair)
                if fair.priority > 127 && !process.realtime_scheduling =>
            {
                return Err(Status::ErrAccessDenied);
            }
            SchedulingProfile::Deadline(_) if !process.realtime_scheduling => {
                return Err(Status::ErrAccessDenied);
            }
            _ => {}
        }
        self.scheduler
            .set_profile(thread_id as u64 + 1, profile)
            .map_err(scheduler_status)?;
        self.changed(incremental::META, 0);
        Ok(())
    }

    pub fn set_thread_cpu_affinity(
        &mut self,
        thread_handle: u64,
        affinity_mask: u64,
    ) -> Result<()> {
        let Object::Thread(thread_id) = self.capability(thread_handle, ADMIN)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        self.scheduler
            .set_cpu_affinity(thread_id as u64 + 1, affinity_mask)
            .map_err(scheduler_status)?;
        self.changed(incremental::META, 0);
        Ok(())
    }

    pub fn yield_current_on_cpu(&mut self, cpu_id: u8, target_thread_handle: u64) -> Result<()> {
        let target_id = if target_thread_handle == 0 {
            None
        } else {
            let Object::Thread(thread_id) = self.capability(target_thread_handle, ADMIN)?.object
            else {
                return Err(Status::ErrInvalidHandle);
            };
            Some(thread_id as u64 + 1)
        };
        self.scheduler
            .yield_current_to_on_cpu(cpu_id, target_id)
            .map_err(scheduler_status)?;
        self.changed(incremental::META, 0);
        Ok(())
    }

    pub fn wait_many(&mut self, items: &[(u64, u32)], deadline_nanos: i64) -> Result<(usize, u32)> {
        for (index, (handle, requested)) in items.iter().enumerate() {
            let observed = self.object_signals(*handle)?;
            if observed & *requested != 0 {
                return Ok((index, observed));
            }
        }
        if deadline_nanos == 0 {
            Err(Status::ErrTimedOut)
        } else {
            let deadline = if deadline_nanos < 0 {
                u64::MAX
            } else {
                deadline_nanos as u64
            };
            if !items.is_empty() {
                let mut wait_items = [(0, 0); MAX_WAIT_MANY_ITEMS];
                for (slot, item) in wait_items.iter_mut().zip(items.iter()) {
                    *slot = *item;
                }
                if let Some(thread) = self.threads.get_mut(self.current_thread) {
                    thread.blocked_wait_many = true;
                    thread.running = false;
                }
                let _ = self.scheduler.block_current(BlockReason::WaitMany {
                    items: wait_items,
                    item_count: items.len().min(MAX_WAIT_MANY_ITEMS) as u8,
                    deadline_nanos: deadline,
                });
                self.changed(incremental::THREAD, self.current_thread);
            }
            Err(Status::ErrTimedOut)
        }
    }

    fn wake_waiters_for_object(&mut self, object: Object, signals: u32) {
        let handles = self
            .handles
            .iter()
            .enumerate()
            .filter_map(|(index, cap)| {
                cap.filter(|capability| capability.object == object)
                    .map(|_| index as u64 + 1)
            })
            .collect::<Vec<_>>();
        for handle in handles {
            self.scheduler.wake_handle_waiters(handle, signals);
        }
        self.clear_unblocked_wait_many_threads();
    }

    fn clear_unblocked_wait_many_threads(&mut self) {
        let mut changed = [0u64; MAX_THREADS.div_ceil(64)];
        for (id, blocked) in self.scheduler.block_states() {
            if !blocked {
                let Some(thread_id) = id.checked_sub(1).map(|n| n as usize) else {
                    continue;
                };
                let Some(thread) = self.threads.get_mut(thread_id) else {
                    continue;
                };
                if !thread.exited && (thread.blocked_wait_many || thread.blocked_futex.is_some()) {
                    thread.blocked_wait_many = false;
                    thread.blocked_futex = None;
                    thread.running = true;
                    changed[thread_id / 64] |= 1 << (thread_id % 64);
                }
            }
        }
        for (group, mut bits) in changed.into_iter().enumerate() {
            while bits != 0 {
                let index = group * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                self.changed(incremental::THREAD, index);
            }
        }
    }
    fn add_thread_to_process(&mut self, process: usize, context: Context) -> Result<usize> {
        if self.threads.len() >= MAX_THREADS {
            return Err(Status::ErrResourceExhausted);
        }
        let id = self.threads.len();
        self.threads.push(Thread {
            process,
            context,
            running: true,
            exited: false,
            blocked_futex: None,
            blocked_wait_many: false,
            exit_code: 0,
            restricted: None,
        });
        let scheduler_id = id as u64 + 1;
        let mut task = SchedulerTask::new(
            scheduler_id,
            process as u64 + 1,
            self.processes[process].resource_group_id,
            1,
            "runtime",
        )
        .map_err(|_| Status::ErrNoMemory)?;
        // The hardware runtime does not yet preserve an EL1 idle continuation
        // when a secondary CPU starts or blocks an EL0 task. Keep runtime
        // threads on CPU 0 until that handoff is implemented.
        task.cpu_affinity_mask = 1;
        self.scheduler
            .add_task(task)
            .map_err(|_| Status::ErrNoMemory)?;
        if self.threads.len() == 1 {
            if let Some(now_ns) = self.backend.monotonic_ns() {
                let _ = self.scheduler.tick_at_on_cpu(0, now_ns);
            }
            self.current_thread = id;
            self.current = process;
        }
        self.changed(PROCESS, process);
        self.changed(incremental::THREAD, id);
        Ok(id)
    }
    pub fn schedule(&mut self, frame: &mut Context) -> AddressSpaceSwitch {
        self.schedule_yield_at_on_cpu(0, self.scheduler.now_ns(), frame)
    }

    pub fn schedule_yield_at_on_cpu(
        &mut self,
        cpu_id: u8,
        now_ns: u64,
        frame: &mut Context,
    ) -> AddressSpaceSwitch {
        if self.processes.is_empty() || self.threads.is_empty() {
            return AddressSpaceSwitch {
                root_table_phys: 0,
                asid: 0,
                userspace_pac_key: PacKeyMaterial::zero(),
            };
        }
        self.save_context(*frame);
        // Entering through the kernel scheduler is an explicit reschedule
        // point (timer expiry, yield, or a blocking syscall), so rotate the
        // current fair task rather than merely sampling accounting state.
        let decision = self
            .scheduler
            .yield_current_to_at_on_cpu(cpu_id, None, now_ns)
            .unwrap_or_else(|_| self.scheduler.tick());
        self.clear_unblocked_wait_many_threads();
        if let Some(next) = decision.next_task_id {
            let thread_id = next as usize - 1;
            if let Some(thread) = self.threads.get(thread_id) {
                self.current_thread = thread_id;
                self.current = thread.process;
                *frame = thread.context;
                return AddressSpaceSwitch {
                    root_table_phys: self.processes[self.current].root,
                    asid: self.processes[self.current].asid,
                    userspace_pac_key: self.processes[self.current].userspace_pac_key,
                };
            }
        }
        AddressSpaceSwitch {
            root_table_phys: self.processes[self.current].root,
            asid: self.processes[self.current].asid,
            userspace_pac_key: self.processes[self.current].userspace_pac_key,
        }
    }

    /// Account and schedule the task executing on `cpu_id`.  The hardware
    /// runtime calls this from that CPU's one-shot timer/SGI path; callers
    /// must not infer ownership from the legacy `current` fields.
    pub fn schedule_at_on_cpu(
        &mut self,
        cpu_id: u8,
        now_ns: u64,
        frame: &mut Context,
    ) -> AddressSpaceSwitch {
        if self.processes.is_empty() || self.threads.is_empty() {
            return AddressSpaceSwitch {
                root_table_phys: 0,
                asid: 0,
                userspace_pac_key: PacKeyMaterial::zero(),
            };
        }
        if self
            .scheduler
            .current_on_cpu(cpu_id)
            .is_some_and(|task| task.id as usize == self.current_thread.saturating_add(1))
        {
            self.save_context(*frame);
        }
        let decision = match self.scheduler.tick_at_on_cpu(cpu_id, now_ns) {
            Ok(decision) => decision,
            Err(_) => return self.current_address_space(),
        };
        self.clear_unblocked_wait_many_threads();
        if let Some(next) = decision.next_task_id {
            let thread_id = next as usize - 1;
            if let Some(thread) = self.threads.get(thread_id) {
                self.current_thread = thread_id;
                self.current = thread.process;
                *frame = thread.context;
                return self.current_address_space();
            }
        }
        self.current_address_space()
    }

    /// Select the syscall/process capability context for a CPU which has
    /// already entered EL1.  This is deliberately derived from Scheduler
    /// ownership instead of trusting a global current-thread slot.
    pub fn bind_current_cpu(&mut self, cpu_id: u8) -> bool {
        let Some(task) = self.scheduler.current_on_cpu(cpu_id) else {
            return false;
        };
        let thread_id = task.id as usize - 1;
        let Some(thread) = self.threads.get(thread_id) else {
            return false;
        };
        self.current_thread = thread_id;
        self.current = thread.process;
        true
    }

    pub fn next_deadline_on_cpu(&self, cpu_id: u8) -> Option<u64> {
        let scheduled = self.scheduler.next_deadline_on_cpu(cpu_id).ok().flatten();
        if cpu_id == 0 && self.has_pending_reclamation() {
            let maintenance = self
                .backend
                .monotonic_ns()
                .unwrap_or(self.scheduler.now_ns())
                .saturating_add(1_000_000);
            Some(scheduled.map_or(maintenance, |deadline| deadline.min(maintenance)))
        } else {
            scheduled
        }
    }

    fn current_address_space(&self) -> AddressSpaceSwitch {
        self.processes
            .get(self.current)
            .map(|process| AddressSpaceSwitch {
                root_table_phys: process.root,
                asid: process.asid,
                userspace_pac_key: process.userspace_pac_key,
            })
            .unwrap_or(AddressSpaceSwitch {
                root_table_phys: 0,
                asid: 0,
                userspace_pac_key: PacKeyMaterial::zero(),
            })
    }
    pub fn exit_current(&mut self) {
        self.exit_current_with_code(0);
    }
    pub fn exit_current_with_code(&mut self, exit_code: i32) {
        // Renderer and driver workers may finish while their process prepares
        // a transplant. Only losing the last thread invalidates that process;
        // an ordinary worker exit must not destroy the replacement candidate.
        let last_thread = !self.threads.iter().enumerate().any(|(id, thread)| {
            id != self.current_thread && thread.process == self.current && !thread.exited
        });
        if last_thread
            && self
                .handover
                .as_ref()
                .is_some_and(|h| h.target == self.current || h.source == self.current)
        {
            let _ = self.abort_handover();
        }
        let restricted_vmo = if let Some(thread) = self.threads.get_mut(self.current_thread) {
            thread.running = false;
            thread.exited = true;
            thread.exit_code = exit_code;
            thread.restricted.take().map(|binding| binding.state_vmo)
        } else {
            None
        };
        if let Some(vmo) = restricted_vmo {
            self.release_vmo(vmo);
        }
        // Aborting a candidate can already remove it from the scheduler and
        // select a successor. Exit the caller by identity, never that successor.
        let _ = self
            .scheduler
            .exit_task(self.current_thread as u64 + 1, exit_code);
        self.changed(incremental::THREAD, self.current_thread);
        self.wake_waiters_for_object(
            Object::Thread(self.current_thread),
            crate::kernel_services::SIGNAL_TERMINATED,
        );
        if !self
            .threads
            .iter()
            .any(|t| t.process == self.current && !t.exited)
        {
            self.retire_process(self.current);
        }
    }
    pub fn futex_wait_current(&mut self, uaddr: u64, expected: u32) -> Result<()> {
        self.futex_wait_current_timeout(uaddr, expected, -1)
    }
    /// A wake, including deadline expiry, is a prompt to recheck the userspace
    /// predicate and monotonic deadline. The response is enqueued before sleep.
    pub fn futex_wait_current_timeout(
        &mut self,
        uaddr: u64,
        expected: u32,
        timeout_ns: i64,
    ) -> Result<()> {
        if uaddr == 0 || uaddr & 3 != 0 || timeout_ns < -1 {
            return Err(Status::ErrInvalidArgs);
        }
        let mut bytes = [0; 4];
        self.copy_from_user(uaddr, &mut bytes)?;
        if u32::from_le_bytes(bytes) != expected {
            return Err(Status::ErrResourceExhausted);
        }
        if timeout_ns == 0 {
            return Err(Status::ErrTimedOut);
        }
        let reason = if timeout_ns < 0 {
            BlockReason::Futex { uaddr }
        } else {
            BlockReason::FutexUntil {
                uaddr,
                deadline_nanos: self
                    .backend
                    .monotonic_ns()
                    .unwrap_or(self.scheduler.now_ns())
                    .saturating_add(timeout_ns as u64),
            }
        };
        let thread = self
            .threads
            .get_mut(self.current_thread)
            .ok_or(Status::ErrInvalidHandle)?;
        thread.blocked_futex = Some(uaddr);
        let _ = self.scheduler.block_current(reason);
        self.changed(incremental::THREAD, self.current_thread);
        Ok(())
    }
    pub fn futex_wake(&mut self, uaddr: u64, wake_count: u32) -> Result<u32> {
        if uaddr == 0 || uaddr & 3 != 0 {
            return Err(Status::ErrInvalidArgs);
        }
        self.clear_unblocked_wait_many_threads();
        let mut woken = 0;
        for thread_id in 0..self.threads.len() {
            if woken == wake_count {
                break;
            }
            let thread = &mut self.threads[thread_id];
            if thread.process == self.current && thread.blocked_futex == Some(uaddr) {
                thread.blocked_futex = None;
                thread.running = true;
                let _ = self.scheduler.wake_task(thread_id as u64 + 1);
                self.changed(incremental::THREAD, thread_id);
                woken += 1;
            }
        }
        Ok(woken)
    }
    pub fn current_thread_blocked(&self) -> bool {
        self.threads.get(self.current_thread).is_some_and(|thread| {
            thread.blocked_futex.is_some()
                || self
                    .scheduler
                    .task(self.current_thread as u64 + 1)
                    .is_some_and(|task| task.block_reason.is_some())
        })
    }

    /// A syscall may retire one thread while its process and siblings remain
    /// alive. Such a caller must never return through its old trap frame.
    pub fn current_thread_can_return(&self) -> bool {
        self.processes
            .get(self.current)
            .is_some_and(|process| process.running && !process.exited)
            && self
                .threads
                .get(self.current_thread)
                .is_some_and(|thread| thread.running && !thread.exited)
            && !self.current_thread_blocked()
    }
}

fn scheduler_status(error: crate::sched::SchedulerError) -> Status {
    match error {
        crate::sched::SchedulerError::Full => Status::ErrNoMemory,
        crate::sched::SchedulerError::InvalidTask => Status::ErrInvalidArgs,
        crate::sched::SchedulerError::AlreadyExists => Status::ErrAlreadyExists,
        crate::sched::SchedulerError::AccessDenied => Status::ErrAccessDenied,
        crate::sched::SchedulerError::ResourceExhausted => Status::ErrResourceExhausted,
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

const fn runtime_deadline_expired(now_nanos: u64, deadline_nanos: i64) -> bool {
    deadline_nanos == 0 || (deadline_nanos > 0 && now_nanos >= deadline_nanos as u64)
}

mod random;
