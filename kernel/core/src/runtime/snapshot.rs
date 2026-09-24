//! Runtime ABI v1. Object identities are vector slots, including vacant slots.
use super::*;
use crate::cpu_features::AsidAllocator;
use crate::runtime::memory::range_contains;
use crate::transplant::{
    TransplantError,
    codec::{Reader, Result, Writer},
};

pub const VERSION: u64 = 21;
const INTERRUPT_VERSION: u64 = 20;
const RESTRICTED_VERSION: u64 = 21;
const CHANNEL_IDENTITY_VERSION: u64 = 19;
const PRE_SLOT_VERSION: u64 = 18;
const SHARED_DEVICE_VERSION: u64 = 17;
const ENTROPY_VERSION: u64 = 15;
const ARCHITECTURE_VERSION: u64 = 16;
const RETIREMENT_VERSION: u64 = 14;
const DMA_VERSION: u64 = 13;
const PROCESS_PAC_VERSION: u64 = 12;
const VMO_BACKING_VERSION: u64 = 11;
const TIME_PAGE_VERSION: u64 = 10;
const PROFILE_VERSION: u64 = 8;
const RESOURCE_GROUP_VERSION: u64 = 7;
const PREVIOUS_VERSION: u64 = 6;
const MAGIC: u64 = u64::from_le_bytes(*b"BEXRT001");

pub fn write_context(w: &mut Writer<'_>, c: &Context) -> Result<()> {
    for v in c.regs {
        w.word(v)?;
    }
    for v in [c.stack_pointer, c.instruction_pointer, c.processor_state] {
        w.word(v)?;
    }
    for v in c.simd {
        w.word(v)?;
    }
    w.word(c.fp_control)?;
    w.word(c.fp_status)?;
    w.word(c.thread_pointer)?;
    w.word(c.architecture)
}
pub fn read_context(r: &mut Reader<'_>) -> Result<Context> {
    let mut context = read_context_legacy(r)?;
    context.architecture = r.word()?;
    if !context.matches_current_architecture() {
        return Err(TransplantError::InvalidRuntimeSnapshot);
    }
    Ok(context)
}
pub(super) fn read_context_legacy(r: &mut Reader<'_>) -> Result<Context> {
    let mut c = Context::zero();
    for v in &mut c.regs {
        *v = r.word()?;
    }
    c.stack_pointer = r.word()?;
    c.instruction_pointer = r.word()?;
    c.processor_state = r.word()?;
    for v in &mut c.simd {
        *v = r.word()?;
    }
    c.fp_control = r.word()?;
    c.fp_status = r.word()?;
    c.thread_pointer = r.word()?;
    Ok(c)
}

pub(super) fn write_restricted_binding(
    w: &mut Writer<'_>,
    binding: Option<RestrictedBinding>,
) -> Result<()> {
    w.word(binding.is_some() as u64)?;
    if let Some(binding) = binding {
        w.word(binding.state_vmo as u64)?;
        write_context(w, &binding.host_context)?;
        write_context(w, &binding.guest_context)?;
        for value in [
            binding.host_readonly_thread_pointer,
            binding.guest_readonly_thread_pointer,
            binding.vector_entry,
            binding.vector_context,
            binding.active as u64,
            binding.pending_kick as u64,
            binding.transition_pending as u64,
        ] {
            w.word(value)?;
        }
    }
    Ok(())
}

pub(super) fn read_restricted_binding(r: &mut Reader<'_>) -> Result<Option<RestrictedBinding>> {
    if !r.flag()? {
        return Ok(None);
    }
    Ok(Some(RestrictedBinding {
        state_vmo: r.index()?,
        host_context: read_context(r)?,
        guest_context: read_context(r)?,
        host_readonly_thread_pointer: r.word()?,
        guest_readonly_thread_pointer: r.word()?,
        vector_entry: r.word()?,
        vector_context: r.word()?,
        active: r.flag()?,
        pending_kick: r.flag()?,
        transition_pending: r.flag()?,
    }))
}

pub fn write_profile(w: &mut Writer<'_>, profile: SchedulingProfile) -> Result<()> {
    match profile {
        SchedulingProfile::Fair(profile) => {
            w.word(1)?;
            w.word(profile.priority as u64)?;
            w.word(profile.weight as u64)?;
        }
        SchedulingProfile::Deadline(profile) => {
            w.word(2)?;
            w.word(profile.capacity_ns)?;
            w.word(profile.deadline_ns)?;
            w.word(profile.period_ns)?;
        }
    }
    Ok(())
}

pub fn read_profile(r: &mut Reader<'_>) -> Result<SchedulingProfile> {
    match r.word()? {
        1 => {
            let priority =
                u8::try_from(r.word()?).map_err(|_| TransplantError::InvalidRuntimeSnapshot)?;
            let weight =
                u32::try_from(r.word()?).map_err(|_| TransplantError::InvalidRuntimeSnapshot)?;
            if priority == 0 || weight == 0 {
                return Err(TransplantError::InvalidRuntimeSnapshot);
            }
            Ok(SchedulingProfile::Fair(FairProfile { priority, weight }))
        }
        2 => {
            let profile = DeadlineProfile {
                capacity_ns: r.word()?,
                deadline_ns: r.word()?,
                period_ns: r.word()?,
            };
            if !profile.valid() {
                return Err(TransplantError::InvalidRuntimeSnapshot);
            }
            Ok(SchedulingProfile::Deadline(profile))
        }
        _ => Err(TransplantError::InvalidRuntimeSnapshot),
    }
}

pub fn write_runtime_message(w: &mut Writer<'_>, message: &Message) -> Result<()> {
    w.word(message.bytes.len() as u64)?;
    w.bytes(&message.bytes)?;
    w.word(message.handles.len() as u64)?;
    for handle in &message.handles {
        w.word(*handle)?;
    }
    Ok(())
}

pub fn read_runtime_message(r: &mut Reader<'_>) -> Result<Message> {
    let len = r.count(65536)?;
    let bytes = r.bytes(len)?.to_vec();
    let mut handles = Vec::new();
    for _ in 0..r.count(64)? {
        handles.push(r.word()?);
    }
    Ok(Message { bytes, handles })
}

impl<B: Backend> Runtime<B> {
    pub fn write_snapshot(&self, w: &mut Writer<'_>) -> Result<()> {
        if self.handover.is_some() {
            return Err(TransplantError::InvalidRuntimeSnapshot);
        }
        w.word(MAGIC)?;
        w.word(VERSION)?;
        w.word(Context::ARCHITECTURE)?;
        w.word(self.current as u64)?;
        w.word(self.bootfs_pages)?;
        w.word(self.reclaimed_pages)?;
        w.word(self.asids.next_asid() as u64)?;
        w.word(self.asids.max_asid() as u64)?;
        w.word(self.realtime_slew.realtime_offset_ns as u64)?;
        w.word(self.realtime_slew.slew_start_monotonic_ns)?;
        w.word(self.realtime_slew.slew_remaining_ns as u64)?;
        w.word(self.realtime_slew.slew_rate_ppm as i64 as u64)?;
        w.word(self.time_page_vmo.unwrap_or(usize::MAX) as u64)?;
        w.word(self.zero_page.unwrap_or(0))?;
        self.write_entropy(w)?;
        w.word(self.processes.len() as u64)?;
        for p in &self.processes {
            w.text(&p.name)?;
            w.text(&p.package)?;
            w.word(p.hardware as u64)?;
            w.word(p.resource_group_id as u64)?;
            w.word(p.realtime_scheduling as u64)?;
            w.word(p.authority as u64)?;
            w.word(p.quarantined as u64)?;
            w.word(p.root)?;
            w.word(p.asid as u64)?;
            w.word(p.userspace_pac_key.lo)?;
            w.word(p.userspace_pac_key.hi)?;
            write_context(w, &p.context)?;
            w.word(p.running as u64)?;
            w.word(p.exited as u64)?;
            w.word(p.next_va)?;
            w.word(p.root_vmar as u64)?;
            w.word(p.heap_vmar as u64)?;
            w.word(p.mappings.len() as u64)?;
            for m in &p.mappings {
                for v in [
                    m.vmo as u64,
                    m.vmar as u64,
                    m.offset,
                    m.va,
                    m.size,
                    m.rights as u64,
                ] {
                    w.word(v)?;
                }
            }
        }
        w.word(self.current_thread as u64)?;
        w.word(self.threads.len() as u64)?;
        for thread in &self.threads {
            w.word(thread.process as u64)?;
            write_context(w, &thread.context)?;
            w.word(thread.running as u64)?;
            w.word(thread.exited as u64)?;
            w.word(thread.blocked_futex.unwrap_or(0))?;
            w.word(thread.blocked_wait_many as u64)?;
            w.word(thread.exit_code as u64)?;
            write_restricted_binding(w, thread.restricted)?;
        }
        w.word(self.vmos.len() as u64)?;
        for v in &self.vmos {
            w.word(v.is_some() as u64)?;
            if let Some(v) = v {
                for n in [v.size, v.refs as u64, v.device as u64, v.bootfs as u64] {
                    w.word(n)?;
                }
                write_vmo_backing(w, &v.backing)?;
            }
        }
        w.word(self.profiles.len() as u64)?;
        for profile in &self.profiles {
            write_profile(w, *profile)?;
        }
        w.word(self.handles.len() as u64)?;
        for cap in &self.handles {
            w.word(cap.is_some() as u64)?;
            if let Some(c) = cap {
                let (kind, id, end) = match c.object {
                    Object::Vmo(id) => (1, id, 0),
                    Object::Vmar(id) => (7, id, 0),
                    Object::Channel(id, end) => (2, id, end as u64),
                    Object::Process(id) => (3, id, 0),
                    Object::Space(id) => (4, id, 0),
                    Object::Thread(id) => (5, id, 0),
                    Object::Socket(id, end) => (6, id, end as u64),
                    Object::Profile(id) => (8, id, 0),
                    Object::ReplyToken(id, end, call_id) => (9, id, (call_id << 1) | end as u64),
                    Object::IommuDomain(id) => (10, id, 0),
                    Object::Interrupt(id) => (11, id, 0),
                };
                for v in [kind, id as u64, end as u64, c.rights as u64, c.owner as u64] {
                    w.word(v)?;
                }
            }
        }
        w.word(self.channels.len() as u64)?;
        for c in &self.channels {
            w.word(c.identity)?;
            w.word(c.next_call_id)?;
            w.word(c.policy.enable_priority_inheritance as u64)?;
            w.word(c.policy.enable_timeslice_donation as u64)?;
            for end in 0..2 {
                w.word(c.refs[end] as u64)?;
                w.word(c.queues[end].len() as u64)?;
                for m in &c.queues[end] {
                    write_runtime_message(w, m)?;
                }
                w.word(c.calls[end].len() as u64)?;
                for call in &c.calls[end] {
                    w.word(call.id)?;
                    w.word(call.caller_end as u64)?;
                    w.word(call.caller_thread as u64)?;
                    w.word(call.serving as u64)?;
                    w.word(call.server_thread.unwrap_or(usize::MAX) as u64)?;
                    write_runtime_message(w, &call.request)?;
                    w.word(call.reply.is_some() as u64)?;
                    if let Some(reply) = &call.reply {
                        write_runtime_message(w, reply)?;
                    }
                }
            }
        }
        w.word(self.sockets.len() as u64)?;
        for s in &self.sockets {
            for end in 0..2 {
                w.word(s.refs[end] as u64)?;
                w.word(s.read_closed[end] as u64)?;
                w.word(s.write_closed[end] as u64)?;
                w.word(s.queues[end].len() as u64)?;
                for bytes in &s.queues[end] {
                    w.word(bytes.len() as u64)?;
                    w.bytes(bytes)?;
                }
            }
        }
        w.word(self.pins.len() as u64)?;
        for p in &self.pins {
            w.word(p.is_some() as u64)?;
            if let Some((owner, id)) = p {
                w.word(*owner as u64)?;
                w.word(*id as u64)?;
            }
        }
        w.word(self.vmars.len() as u64)?;
        for vmar in &self.vmars {
            w.word(vmar.is_some() as u64)?;
            if let Some(vmar) = vmar {
                for n in [
                    vmar.owner as u64,
                    vmar.parent.unwrap_or(usize::MAX) as u64,
                    vmar.base,
                    vmar.size,
                    vmar.rights as u64,
                    vmar.refs as u64,
                    vmar.destroyed as u64,
                ] {
                    w.word(n)?;
                }
            }
        }
        w.word(self.iommu_domains.len() as u64)?;
        for domain in &self.iommu_domains {
            dma_snapshot::write_domain(w, *domain)?;
        }
        w.word(self.dma_mappings.len() as u64)?;
        for mapping in &self.dma_mappings {
            dma_snapshot::write_mapping(w, *mapping)?;
        }
        w.word(self.interrupts.len() as u64)?;
        for interrupt in &self.interrupts {
            w.word(interrupt.is_some() as u64)?;
            if let Some(interrupt) = interrupt {
                for value in [
                    u64::from(interrupt.irq_number),
                    u64::from(interrupt.flags),
                    interrupt.masked as u64,
                    interrupt.awaiting_ack as u64,
                    interrupt.window_started_ns,
                    u64::from(interrupt.events_in_window),
                    u64::from(interrupt.flood_limit_per_second),
                ] {
                    w.word(value)?;
                }
            }
        }
        self.scheduler.write_snapshot(w)?;
        Ok(())
    }

    pub fn read_snapshot(backend: B, r: &mut Reader<'_>) -> Result<Self> {
        if r.word()? != MAGIC {
            return Err(TransplantError::BadMagic);
        }
        let version = r.word()?;
        if version != VERSION
            && version != INTERRUPT_VERSION
            && version != CHANNEL_IDENTITY_VERSION
            && version != PRE_SLOT_VERSION
            && version != SHARED_DEVICE_VERSION
            && version != ARCHITECTURE_VERSION
            && version != ENTROPY_VERSION
            && version != RETIREMENT_VERSION
            && version != DMA_VERSION
            && version != PROCESS_PAC_VERSION
            && version != PROFILE_VERSION
            && version != RESOURCE_GROUP_VERSION
            && version != PREVIOUS_VERSION
        {
            return Err(TransplantError::UnsupportedVersion);
        }
        let architecture = if version >= ARCHITECTURE_VERSION {
            r.word()?
        } else {
            1
        };
        if architecture != Context::ARCHITECTURE {
            return Err(TransplantError::InvalidRuntimeSnapshot);
        }
        let mut rt = Self::new(backend);
        rt.current = r.index()?;
        rt.bootfs_pages = r.word()?;
        rt.reclaimed_pages = r.word()?;
        let next_asid =
            u16::try_from(r.word()?).map_err(|_| TransplantError::InvalidRuntimeSnapshot)?;
        let max_asid =
            u16::try_from(r.word()?).map_err(|_| TransplantError::InvalidRuntimeSnapshot)?;
        rt.asids = AsidAllocator::restore(next_asid, max_asid);
        if version >= TIME_PAGE_VERSION {
            rt.realtime_slew.realtime_offset_ns = r.word()? as i64;
            rt.realtime_slew.slew_start_monotonic_ns = r.word()?;
            rt.realtime_slew.slew_remaining_ns = r.word()? as i64;
            rt.realtime_slew.slew_rate_ppm = r.word()? as i64 as i32;
            rt.time_page_vmo = match r.word()? {
                value if value == usize::MAX as u64 => None,
                value => Some(
                    usize::try_from(value).map_err(|_| TransplantError::InvalidRuntimeSnapshot)?,
                ),
            };
            if version >= VMO_BACKING_VERSION {
                rt.zero_page = match r.word()? {
                    0 => None,
                    value => Some(value),
                };
            }
        }
        if version >= ENTROPY_VERSION {
            rt.read_entropy(r)?;
        }
        for _ in 0..r.count(32)? {
            let name = r.text(64)?.to_string();
            let package = r.text(96)?.to_string();
            let hardware = r.word()?;
            if hardware > 2 {
                return Err(TransplantError::InvalidRuntimeSnapshot);
            }
            let resource_group_id = if version >= RESOURCE_GROUP_VERSION {
                u32::try_from(r.word()?).map_err(|_| TransplantError::InvalidRuntimeSnapshot)?
            } else {
                1
            };
            let realtime_scheduling = if version >= RESOURCE_GROUP_VERSION {
                r.flag()?
            } else {
                false
            };
            if resource_group_id == 0 {
                return Err(TransplantError::InvalidRuntimeSnapshot);
            }
            let authority = r.word()?;
            if authority & !u64::from(handover::AUTH_VALID_MASK) != 0 {
                return Err(TransplantError::InvalidRuntimeSnapshot);
            }
            let quarantined = r.flag()?;
            if quarantined {
                return Err(TransplantError::InvalidRuntimeSnapshot);
            }
            let root = r.word()?;
            let asid =
                u16::try_from(r.word()?).map_err(|_| TransplantError::InvalidRuntimeSnapshot)?;
            let userspace_pac_key = if version >= PROCESS_PAC_VERSION {
                PacKeyMaterial {
                    lo: r.word()?,
                    hi: r.word()?,
                }
            } else {
                PacKeyMaterial::zero()
            };
            let context = if version >= ARCHITECTURE_VERSION {
                read_context(r)?
            } else {
                read_context_legacy(r)?
            };
            let running = r.flag()?;
            let exited = r.flag()?;
            let next_va = r.word()?;
            let root_vmar = r.index()?;
            let heap_vmar = r.index()?;
            let mut mappings = Vec::new();
            for _ in 0..r.count(65536)? {
                mappings.push(Mapping {
                    vmo: r.index()?,
                    vmar: r.index()?,
                    offset: r.word()?,
                    va: r.word()?,
                    size: r.word()?,
                    rights: read_rights(r)?,
                });
            }
            rt.processes.push(Process {
                authority: authority as u32,
                quarantined,
                name,
                package,
                hardware: hardware as u32,
                resource_group_id,
                realtime_scheduling,
                root,
                asid,
                userspace_pac_key,
                context,
                running,
                exited,
                root_vmar,
                heap_vmar,
                mappings,
                next_va,
            });
        }
        rt.current_thread = r.index()?;
        for _ in 0..r.count(MAX_THREADS)? {
            rt.threads.push(Thread {
                process: r.index()?,
                context: if version >= ARCHITECTURE_VERSION {
                    read_context(r)?
                } else {
                    read_context_legacy(r)?
                },
                running: r.flag()?,
                exited: r.flag()?,
                blocked_futex: match r.word()? {
                    0 => None,
                    value => Some(value),
                },
                blocked_wait_many: r.flag().unwrap_or(false),
                exit_code: r.word()? as i32,
                restricted: if version >= RESTRICTED_VERSION {
                    read_restricted_binding(r)?
                } else {
                    None
                },
            });
        }
        for _ in 0..r.count(262144)? {
            rt.vmos.push(if r.flag()? {
                if version >= VMO_BACKING_VERSION {
                    Some(Vmo {
                        size: r.word()?,
                        refs: r.index()?,
                        device: r.flag()?,
                        bootfs: r.flag()?,
                        backing: read_vmo_backing(r)?,
                    })
                } else {
                    Some(Vmo {
                        backing: VmoBacking::Contiguous { base: r.word()? },
                        size: r.word()?,
                        refs: r.index()?,
                        device: r.flag()?,
                        bootfs: r.flag()?,
                    })
                }
            } else {
                None
            });
        }
        if version >= PROFILE_VERSION {
            for _ in 0..r.count(262144)? {
                rt.profiles.push(read_profile(r)?);
            }
        }
        for _ in 0..r.count(262144)? {
            rt.handles.push(if r.flag()? {
                let kind = r.word()?;
                let id = r.index()?;
                let end = r.index()?;
                let object = match (kind, end) {
                    (1, 0) => Object::Vmo(id),
                    (7, 0) => Object::Vmar(id),
                    (2, 0..=1) => Object::Channel(id, end),
                    (3, 0) => Object::Process(id),
                    (4, 0) => Object::Space(id),
                    (5, 0) => Object::Thread(id),
                    (6, 0..=1) => Object::Socket(id, end),
                    (8, 0) => Object::Profile(id),
                    (9, value) => Object::ReplyToken(id, value & 1, (value >> 1) as u64),
                    (10, 0) => Object::IommuDomain(id),
                    (11, 0) if version >= INTERRUPT_VERSION => Object::Interrupt(id),
                    _ => return Err(TransplantError::InvalidRuntimeSnapshot),
                };
                Some(Capability {
                    object,
                    rights: read_rights(r)?,
                    owner: r.index()?,
                })
            } else {
                None
            });
        }
        for _ in 0..r.count(65536)? {
            let mut c = Channel {
                identity: if version >= CHANNEL_IDENTITY_VERSION {
                    r.word()?
                } else {
                    rt.channels.len() as u64 * 2 + 1
                },
                queues: [VecDeque::new(), VecDeque::new()],
                calls: [VecDeque::new(), VecDeque::new()],
                refs: [0, 0],
                next_call_id: 1,
                policy: ChannelPolicy::disabled(),
            };
            if version >= RETIREMENT_VERSION {
                c.next_call_id = r.word()?;
                c.policy.enable_priority_inheritance = r.flag()?;
                c.policy.enable_timeslice_donation = r.flag()?;
            }
            for end in 0..2 {
                c.refs[end] = r.index()?;
                for _ in 0..r.count(64)? {
                    c.queues[end].push_back(read_runtime_message(r)?);
                }
                if version >= RETIREMENT_VERSION {
                    for _ in 0..r.count(64)? {
                        let id = r.word()?;
                        let caller_end = r.index()?;
                        if caller_end > 1 {
                            return Err(TransplantError::InvalidRuntimeSnapshot);
                        }
                        let caller_thread = r.index()?;
                        let serving = r.flag()?;
                        let server_thread = match r.word()? {
                            value if value == usize::MAX as u64 => None,
                            value => Some(
                                usize::try_from(value)
                                    .map_err(|_| TransplantError::InvalidRuntimeSnapshot)?,
                            ),
                        };
                        let request = read_runtime_message(r)?;
                        let reply = if r.flag()? {
                            Some(read_runtime_message(r)?)
                        } else {
                            None
                        };
                        c.calls[end].push_back(RuntimeCall {
                            id,
                            caller_end,
                            caller_thread,
                            request,
                            reply,
                            serving,
                            server_thread,
                        });
                    }
                }
            }
            rt.channels.push(c);
        }
        for _ in 0..r.count(65536)? {
            let mut s = Socket {
                queues: [VecDeque::new(), VecDeque::new()],
                refs: [0, 0],
                read_closed: [false, false],
                write_closed: [false, false],
            };
            for end in 0..2 {
                s.refs[end] = r.index()?;
                s.read_closed[end] = r.flag()?;
                s.write_closed[end] = r.flag()?;
                for _ in 0..r.count(64)? {
                    let len = r.count(65536)?;
                    s.queues[end].push_back(r.bytes(len)?.to_vec());
                }
            }
            rt.sockets.push(s);
        }
        for _ in 0..r.count(262144)? {
            rt.pins.push(if r.flag()? {
                Some((r.index()?, r.index()?))
            } else {
                None
            });
        }
        for _ in 0..r.count(262144)? {
            rt.vmars.push(if r.flag()? {
                let owner = r.index()?;
                let parent = match r.word()? {
                    value if value == usize::MAX as u64 => None,
                    value => Some(
                        usize::try_from(value)
                            .map_err(|_| TransplantError::InvalidRuntimeSnapshot)?,
                    ),
                };
                Some(Vmar {
                    owner,
                    parent,
                    base: r.word()?,
                    size: r.word()?,
                    rights: read_rights(r)?,
                    refs: r.index()?,
                    destroyed: r.flag()?,
                })
            } else {
                None
            });
        }
        if version >= DMA_VERSION {
            for _ in 0..r.count(262144)? {
                rt.iommu_domains.push(dma_snapshot::read_domain(r)?);
            }
            for _ in 0..r.count(262144)? {
                rt.dma_mappings.push(dma_snapshot::read_mapping(r)?);
            }
            if version >= INTERRUPT_VERSION {
                for _ in 0..r.count(65536)? {
                    rt.interrupts.push(if r.flag()? {
                        let irq_number = u32::try_from(r.word()?)
                            .map_err(|_| TransplantError::InvalidRuntimeSnapshot)?;
                        let flags = u32::try_from(r.word()?)
                            .map_err(|_| TransplantError::InvalidRuntimeSnapshot)?;
                        let masked = r.flag()?;
                        let awaiting_ack = r.flag()?;
                        let window_started_ns = r.word()?;
                        let events_in_window = u32::try_from(r.word()?)
                            .map_err(|_| TransplantError::InvalidRuntimeSnapshot)?;
                        let flood_limit_per_second = u32::try_from(r.word()?)
                            .map_err(|_| TransplantError::InvalidRuntimeSnapshot)?;
                        if flood_limit_per_second == 0 || (!masked && awaiting_ack) {
                            return Err(TransplantError::InvalidRuntimeSnapshot);
                        }
                        Some(Interrupt {
                            irq_number,
                            flags,
                            masked,
                            awaiting_ack,
                            window_started_ns,
                            events_in_window,
                            flood_limit_per_second,
                        })
                    } else {
                        None
                    });
                }
            }
            rt.scheduler = Scheduler::read_snapshot(r)?;
        }
        if version < RETIREMENT_VERSION && rt.vmos.iter().flatten().any(|v| v.refs == 0) {
            return Err(TransplantError::InvalidRuntimeSnapshot);
        }
        rt.validate_snapshot()?;
        if version >= DMA_VERSION {
            return Ok(rt);
        }
        // v6 snapshots predate serialized scheduler queues.  Reconstruct a
        // single-owner runnable view from the preserved thread records; v7
        // additionally preserves each process's resource scheduling policy.
        for process in &rt.processes {
            rt.scheduler
                .ensure_resource_group_with_limits(
                    process.resource_group_id,
                    (process.resource_group_id != 1).then_some(1),
                    1,
                    0,
                    process.realtime_scheduling,
                )
                .map_err(|_| TransplantError::InvalidRuntimeSnapshot)?;
        }
        for (thread_id, thread) in rt.threads.iter().enumerate() {
            if thread.exited
                || !thread.running
                || thread.blocked_futex.is_some()
                || thread.blocked_wait_many
            {
                continue;
            }
            let process = rt
                .processes
                .get(thread.process)
                .ok_or(TransplantError::InvalidRuntimeSnapshot)?;
            let task = SchedulerTask::new(
                thread_id as u64 + 1,
                thread.process as u64 + 1,
                process.resource_group_id,
                1,
                "runtime",
            )
            .map_err(|_| TransplantError::InvalidRuntimeSnapshot)?;
            rt.scheduler
                .add_task(task)
                .map_err(|_| TransplantError::InvalidRuntimeSnapshot)?;
        }
        Ok(rt)
    }

    pub(super) fn validate_snapshot(&self) -> Result<()> {
        let bad = TransplantError::InvalidRuntimeSnapshot;
        if self.current >= self.processes.len()
            || (!self.threads.is_empty() && self.current_thread >= self.threads.len())
        {
            return Err(bad);
        }
        if !self.threads.is_empty() && self.threads[self.current_thread].process != self.current {
            return Err(bad);
        }
        let mut refs = alloc::vec![0usize; self.vmos.len()];
        let mut vmar_refs = alloc::vec![0usize; self.vmars.len()];
        let mut channel_refs = alloc::vec![[0usize; 2]; self.channels.len()];
        let mut socket_refs = alloc::vec![[0usize; 2]; self.sockets.len()];
        let mut transit = alloc::vec![false; self.handles.len()];
        let valid_vmo = |id: usize| self.vmos.get(id).and_then(|v| v.as_ref()).ok_or(bad);
        for (pid, p) in self.processes.iter().enumerate() {
            if (!p.exited && p.root == 0) || p.root % 4096 != 0 || !p.context.valid_user_stack() {
                return Err(bad);
            }
            let root_vmar = self
                .vmars
                .get(p.root_vmar)
                .and_then(|v| v.as_ref())
                .ok_or(bad)?;
            let heap_vmar = self
                .vmars
                .get(p.heap_vmar)
                .and_then(|v| v.as_ref())
                .ok_or(bad)?;
            if root_vmar.owner != pid
                || root_vmar.parent.is_some()
                || heap_vmar.owner != pid
                || heap_vmar.parent != Some(p.root_vmar)
            {
                return Err(bad);
            }
            for m in &p.mappings {
                let v = valid_vmo(m.vmo)?;
                let vmar = self.vmars.get(m.vmar).and_then(|v| v.as_ref()).ok_or(bad)?;
                if m.size == 0
                    || m.size % 4096 != 0
                    || m.va % 4096 != 0
                    || m.offset % 4096 != 0
                    || m.offset.checked_add(m.size).is_none_or(|end| end > v.size)
                    || m.va.checked_add(m.size).is_none()
                    || vmar.owner != pid
                    || !range_contains(vmar.base, vmar.size, m.va, m.size)
                {
                    return Err(bad);
                }
                refs[m.vmo] += 1;
            }
        }
        for thread in &self.threads {
            if thread.process >= self.processes.len()
                || (!thread.exited && !thread.context.valid_user_stack())
            {
                return Err(bad);
            }
            if let Some(binding) = thread.restricted {
                let vmo = valid_vmo(binding.state_vmo)?;
                if thread.exited
                    || vmo.size != bexos_restricted_abi::STATE_VMO_SIZE
                    || vmo.device
                    || vmo.bootfs
                    || binding.vector_entry >= bexos_boot::USER_END
                    || binding.host_context.architecture != Context::ARCHITECTURE
                    || binding.guest_context.architecture != Context::ARCHITECTURE
                {
                    return Err(bad);
                }
                refs[binding.state_vmo] += 1;
            }
        }
        for c in self.handles.iter().flatten() {
            if c.owner != IN_TRANSIT && c.owner >= self.processes.len() {
                return Err(bad);
            }
            match c.object {
                Object::Vmo(id) => {
                    valid_vmo(id)?;
                    refs[id] += 1;
                }
                Object::Vmar(id) => {
                    let vmar = self.vmars.get(id).and_then(|v| v.as_ref()).ok_or(bad)?;
                    if vmar.destroyed {
                        return Err(bad);
                    }
                    vmar_refs[id] += 1;
                }
                Object::Channel(id, end) => {
                    *channel_refs
                        .get_mut(id)
                        .and_then(|c| c.get_mut(end))
                        .ok_or(bad)? += 1;
                }
                Object::Socket(id, end) => {
                    *socket_refs
                        .get_mut(id)
                        .and_then(|s| s.get_mut(end))
                        .ok_or(bad)? += 1;
                }
                Object::Process(id) | Object::Space(id) => {
                    if id >= self.processes.len() {
                        return Err(bad);
                    }
                }
                Object::Thread(id) => {
                    if id >= self.threads.len() {
                        return Err(bad);
                    }
                }
                Object::Profile(id) => {
                    if id >= self.profiles.len() {
                        return Err(bad);
                    }
                }
                Object::ReplyToken(id, end, _) => {
                    if self.channels.get(id).is_none() || end > 1 {
                        return Err(bad);
                    }
                }
                Object::IommuDomain(id) => {
                    let domain = self
                        .iommu_domains
                        .get(id)
                        .and_then(|domain| domain.as_ref())
                        .ok_or(bad)?;
                    if domain.closed || domain.owner >= self.processes.len() {
                        return Err(bad);
                    }
                }
                Object::Interrupt(id) => {
                    if self
                        .interrupts
                        .get(id)
                        .and_then(|interrupt| interrupt.as_ref())
                        .is_none()
                    {
                        return Err(bad);
                    }
                }
            }
        }
        for mapping in self.dma_mappings.iter().flatten() {
            if mapping.owner >= self.processes.len()
                || self
                    .iommu_domains
                    .get(mapping.domain)
                    .and_then(|domain| domain.as_ref())
                    .is_none()
            {
                return Err(bad);
            }
            valid_vmo(mapping.vmo)?;
            refs[mapping.vmo] += 1;
        }
        for (owner, id) in self.pins.iter().flatten() {
            if *owner >= self.processes.len() {
                return Err(bad);
            }
            valid_vmo(*id)?;
            refs[*id] += 1;
        }
        for (id, c) in self.channels.iter().enumerate() {
            if c.identity == 0
                || c.identity % 2 != 1
                || c.identity == u64::MAX
                || self.channels[..id]
                    .iter()
                    .any(|prior| prior.identity == c.identity)
                || channel_refs[id] != c.refs
            {
                return Err(bad);
            }
            for m in c.queues.iter().flatten() {
                for h in &m.handles {
                    let id = h.checked_sub(1).ok_or(bad)? as usize;
                    let cap = self.handles.get(id).and_then(|c| c.as_ref()).ok_or(bad)?;
                    if cap.owner != IN_TRANSIT || transit[id] {
                        return Err(bad);
                    }
                    transit[id] = true;
                }
            }
        }
        for (id, s) in self.sockets.iter().enumerate() {
            if socket_refs[id] != s.refs {
                return Err(bad);
            }
        }
        for (id, c) in self.handles.iter().enumerate() {
            if c.is_some_and(|c| c.owner == IN_TRANSIT) != transit[id] {
                return Err(bad);
            }
        }
        for (id, v) in self.vmos.iter().enumerate() {
            if let Some(v) = v {
                if v.refs != refs[id]
                    || (v.refs == 0
                        && (v.device || !matches!(v.backing, VmoBacking::LazyAnonymous { .. })))
                    || v.size == 0
                    || v.size % 4096 != 0
                    || !valid_vmo_backing(v)
                {
                    return Err(bad);
                }
            }
        }
        for (id, vmar) in self.vmars.iter().enumerate() {
            if let Some(vmar) = vmar {
                if vmar.refs != vmar_refs[id]
                    || vmar.base % 4096 != 0
                    || vmar.size == 0
                    || vmar.size % 4096 != 0
                    || vmar.base.checked_add(vmar.size).is_none()
                    || vmar.owner >= self.processes.len()
                    || vmar.parent.is_some_and(|parent| {
                        self.vmars.get(parent).and_then(|v| v.as_ref()).is_none()
                    })
                {
                    return Err(bad);
                }
            }
        }
        Ok(())
    }
}
fn read_rights(r: &mut Reader<'_>) -> Result<u32> {
    u32::try_from(r.word()?).map_err(|_| TransplantError::InvalidRuntimeSnapshot)
}

fn write_vmo_backing(w: &mut Writer<'_>, backing: &VmoBacking) -> Result<()> {
    match backing {
        VmoBacking::Contiguous { base } => {
            w.word(1)?;
            w.word(*base)?;
        }
        VmoBacking::SharedDevice { base } => {
            w.word(3)?;
            w.word(*base)?;
        }
        VmoBacking::LazyAnonymous { pages } => {
            w.word(2)?;
            w.word(pages.len() as u64)?;
            for page in pages {
                w.word(page.unwrap_or(0))?;
            }
        }
    }
    Ok(())
}

fn read_vmo_backing(r: &mut Reader<'_>) -> Result<VmoBacking> {
    match r.word()? {
        1 => Ok(VmoBacking::Contiguous { base: r.word()? }),
        3 => Ok(VmoBacking::SharedDevice { base: r.word()? }),
        2 => {
            let mut pages = Vec::new();
            for _ in 0..r.count(262144)? {
                pages.push(match r.word()? {
                    0 => None,
                    value => Some(value),
                });
            }
            Ok(VmoBacking::LazyAnonymous { pages })
        }
        _ => Err(TransplantError::InvalidRuntimeSnapshot),
    }
}

fn valid_vmo_backing(vmo: &Vmo) -> bool {
    match &vmo.backing {
        VmoBacking::Contiguous { base } => {
            *base % 4096 == 0 && base.checked_add(vmo.size).is_some()
        }
        VmoBacking::SharedDevice { base } => {
            vmo.device && !vmo.bootfs && *base % 4096 == 0 && base.checked_add(vmo.size).is_some()
        }
        VmoBacking::LazyAnonymous { pages } => {
            pages.len() as u64 == vmo.size / 4096
                && pages
                    .iter()
                    .flatten()
                    .all(|base| *base % 4096 == 0 && base.checked_add(4096).is_some())
        }
    }
}
