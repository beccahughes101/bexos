//! Typed runtime records used by both bulk copying and coalesced final deltas.
use super::snapshot::{
    read_context, read_context_legacy, read_profile, read_runtime_message, write_context,
    write_profile, write_runtime_message,
};
use super::*;
use crate::cpu_features::AsidAllocator;
use crate::transplant::{
    TransplantError,
    codec::{Reader, Result as CodecResult, Writer},
};
use bexos_migration::dirty::DirtySet;

const RECORD_MAGIC: u64 = u64::from_le_bytes(*b"BEXREC02");
const RECORD_VERSION: u64 = 2;

pub const META: u8 = 0;
pub const PROCESS: u8 = 1;
pub const VMO: u8 = 2;
pub const HANDLE: u8 = 3;
pub const CHANNEL: u8 = 4;
pub const PIN: u8 = 5;
pub const SOCKET: u8 = 6;
pub const THREAD: u8 = 7;
pub const VMAR: u8 = 8;
pub const PROFILE: u8 = 9;
pub const IOMMU_DOMAIN: u8 = 10;
pub const DMA_MAPPING: u8 = 11;
pub const fn key(kind: u8, index: usize) -> u64 {
    ((kind as u64) << 56) | index as u64
}
pub fn parts(key: u64) -> (u8, usize) {
    ((key >> 56) as u8, (key & 0x00ff_ffff_ffff_ffff) as usize)
}

pub struct BulkCursor {
    counts: [usize; 12],
    kind: usize,
    index: usize,
}
impl BulkCursor {
    pub fn next(&mut self) -> Option<u64> {
        while self.kind < self.counts.len() {
            if self.index < self.counts[self.kind] {
                let key = key(self.kind as u8, self.index);
                self.index += 1;
                return Some(key);
            }
            self.kind += 1;
            self.index = 0;
        }
        None
    }
}
impl<B: Backend> Runtime<B> {
    pub fn begin_live_snapshot(&mut self, dirty_capacity: usize) -> Result<BulkCursor> {
        if self.handover.is_some() || self.dirty.is_some() {
            return Err(Status::ErrAlreadyExists);
        }
        self.dirty = Some(DirtySet::new(dirty_capacity));
        Ok(BulkCursor {
            counts: [
                1,
                self.processes.len(),
                self.vmos.len(),
                self.handles.len(),
                self.channels.len(),
                self.pins.len(),
                self.sockets.len(),
                self.threads.len(),
                self.vmars.len(),
                self.profiles.len(),
                self.iommu_domains.len(),
                self.dma_mappings.len(),
            ],
            kind: 0,
            index: 0,
        })
    }
    pub fn end_live_snapshot(&mut self) {
        self.dirty = None;
    }
    pub fn dirty_next(&self) -> core::result::Result<Option<u64>, bexos_migration::Error> {
        self.dirty
            .as_ref()
            .ok_or(bexos_migration::Error::BadState)?
            .next()
    }
    pub fn record_copied(&mut self, key: u64) {
        if let Some(dirty) = &mut self.dirty {
            dirty.copied(key);
        }
    }
    pub(super) fn changed(&mut self, kind: u8, index: usize) {
        if let Some(dirty) = &mut self.dirty {
            dirty.mark(key(kind, index));
            dirty.mark(key(META, 0));
        }
    }
    pub fn save_context(&mut self, context: Context) {
        if let Some(thread) = self.threads.get_mut(self.current_thread) {
            thread.context = context;
            self.changed(THREAD, self.current_thread);
        }
        self.processes[self.current].context = context;
        self.changed(PROCESS, self.current);
    }
    pub fn write_record(&self, record: u64, w: &mut Writer<'_>) -> CodecResult<()> {
        let bad = TransplantError::InvalidRuntimeSnapshot;
        let (kind, index) = parts(record);
        w.word(RECORD_MAGIC)?;
        w.word(RECORD_VERSION)?;
        w.word(Context::ARCHITECTURE)?;
        w.word(record)?;
        match kind {
            META if index == 0 => {
                for v in [
                    self.current as u64,
                    self.bootfs_pages,
                    self.reclaimed_pages,
                    self.asids.next_asid() as u64,
                    self.asids.max_asid() as u64,
                    self.current_thread as u64,
                    self.realtime_slew.realtime_offset_ns as u64,
                    self.realtime_slew.slew_start_monotonic_ns,
                    self.realtime_slew.slew_remaining_ns as u64,
                    self.realtime_slew.slew_rate_ppm as i64 as u64,
                    self.time_page_vmo.unwrap_or(usize::MAX) as u64,
                    self.zero_page.unwrap_or(0),
                ] {
                    w.word(v)?;
                }
                self.scheduler.write_snapshot(w)?;
                self.write_entropy(w)?;
            }
            PROCESS => {
                let p = self.processes.get(index).ok_or(bad)?;
                w.text(&p.name)?;
                w.text(&p.package)?;
                for v in [
                    p.hardware as u64,
                    p.resource_group_id as u64,
                    p.realtime_scheduling as u64,
                    p.authority as u64,
                    p.quarantined as u64,
                    p.root,
                    p.asid as u64,
                    p.userspace_pac_key.lo,
                    p.userspace_pac_key.hi,
                ] {
                    w.word(v)?;
                }
                write_context(w, &p.context)?;
                for v in [
                    p.running as u64,
                    p.exited as u64,
                    p.next_va,
                    p.root_vmar as u64,
                    p.heap_vmar as u64,
                    p.mappings.len() as u64,
                ] {
                    w.word(v)?;
                }
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
            VMO => {
                let v = self.vmos.get(index).ok_or(bad)?;
                w.word(v.is_some() as u64)?;
                if let Some(v) = v {
                    for n in [v.size, v.refs as u64, v.device as u64, v.bootfs as u64] {
                        w.word(n)?;
                    }
                    write_vmo_backing(w, &v.backing)?;
                }
            }
            HANDLE => {
                let cap = self.handles.get(index).ok_or(bad)?;
                w.word(cap.is_some() as u64)?;
                if let Some(c) = cap {
                    let (kind, id, end) = match c.object {
                        Object::Vmo(id) => (1, id, 0),
                        Object::Vmar(id) => (7, id, 0),
                        Object::Channel(id, end) => (2, id, end as u64),
                        Object::Socket(id, end) => (6, id, end as u64),
                        Object::Process(id) => (3, id, 0),
                        Object::Space(id) => (4, id, 0),
                        Object::Thread(id) => (5, id, 0),
                        Object::Profile(id) => (8, id, 0),
                        Object::ReplyToken(id, end, call_id) => {
                            (9, id, (call_id << 1) | end as u64)
                        }
                        Object::IommuDomain(id) => (10, id, 0),
                    };
                    for v in [kind, id as u64, end as u64, c.rights as u64, c.owner as u64] {
                        w.word(v)?;
                    }
                }
            }
            CHANNEL => {
                let c = self.channels.get(index).ok_or(bad)?;
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
                w.word(c.identity)?;
            }
            SOCKET => {
                let s = self.sockets.get(index).ok_or(bad)?;
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
            PIN => {
                let pin = self.pins.get(index).ok_or(bad)?;
                w.word(pin.is_some() as u64)?;
                if let Some((owner, id)) = pin {
                    w.word(*owner as u64)?;
                    w.word(*id as u64)?;
                }
            }
            THREAD => {
                let thread = self.threads.get(index).ok_or(bad)?;
                w.word(thread.process as u64)?;
                write_context(w, &thread.context)?;
                w.word(thread.running as u64)?;
                w.word(thread.exited as u64)?;
                w.word(thread.blocked_futex.unwrap_or(0))?;
                w.word(thread.blocked_wait_many as u64)?;
                w.word(thread.exit_code as u64)?;
            }
            VMAR => {
                let vmar = self.vmars.get(index).ok_or(bad)?;
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
            PROFILE => {
                let profile = self.profiles.get(index).ok_or(bad)?;
                write_profile(w, *profile)?;
            }
            IOMMU_DOMAIN => {
                dma_snapshot::write_domain(w, *self.iommu_domains.get(index).ok_or(bad)?)?
            }
            DMA_MAPPING => {
                dma_snapshot::write_mapping(w, *self.dma_mappings.get(index).ok_or(bad)?)?
            }
            _ => return Err(bad),
        }
        Ok(())
    }

    /// Reconstruct owned objects without invoking resource operations. Runtime
    /// drop frees metadata only, so an aborted candidate cannot release live RAM.
    pub fn adopt_record(&mut self, bytes: &[u8]) -> CodecResult<()> {
        let bad = TransplantError::InvalidRuntimeSnapshot;
        let mut r = Reader::new(bytes);
        let first = r.word()?;
        let legacy = first != RECORD_MAGIC;
        let record = if legacy {
            if Context::ARCHITECTURE != 1 {
                return Err(bad);
            }
            first
        } else {
            if r.word()? != RECORD_VERSION || r.word()? != Context::ARCHITECTURE {
                return Err(bad);
            }
            r.word()?
        };
        let read_saved_context = |r: &mut Reader<'_>| {
            if legacy {
                read_context_legacy(r)
            } else {
                read_context(r)
            }
        };
        let (kind, index) = parts(record);
        match kind {
            META if index == 0 => {
                self.current = r.index()?;
                self.bootfs_pages = r.word()?;
                self.reclaimed_pages = r.word()?;
                let next_asid = u16::try_from(r.word()?).map_err(|_| bad)?;
                let max_asid = u16::try_from(r.word()?).map_err(|_| bad)?;
                self.asids = AsidAllocator::restore(next_asid, max_asid);
                self.current_thread = r.index()?;
                self.realtime_slew.realtime_offset_ns = r.word()? as i64;
                self.realtime_slew.slew_start_monotonic_ns = r.word()?;
                self.realtime_slew.slew_remaining_ns = r.word()? as i64;
                self.realtime_slew.slew_rate_ppm = r.word()? as i64 as i32;
                self.time_page_vmo = match r.word()? {
                    value if value == usize::MAX as u64 => None,
                    value => Some(usize::try_from(value).map_err(|_| bad)?),
                };
                self.zero_page = match r.word()? {
                    0 => None,
                    value => Some(value),
                };
                self.scheduler = Scheduler::read_snapshot(&mut r)?;
                if !r.finished() {
                    self.read_entropy(&mut r)?;
                }
            }
            PROCESS => {
                let name = r.text(64)?.to_string();
                let package = r.text(96)?.to_string();
                let hardware = r.word()?;
                let resource_group_id = u32::try_from(r.word()?).map_err(|_| bad)?;
                let realtime_scheduling = r.flag()?;
                let authority = r.word()?;
                let quarantined = r.flag()?;
                if hardware > 2
                    || authority & !u64::from(handover::AUTH_VALID_MASK) != 0
                    || resource_group_id == 0
                {
                    return Err(bad);
                }
                let root = r.word()?;
                let asid = u16::try_from(r.word()?).map_err(|_| bad)?;
                let userspace_pac_key = PacKeyMaterial {
                    lo: r.word()?,
                    hi: r.word()?,
                };
                let context = read_saved_context(&mut r)?;
                let running = r.flag()?;
                let exited = r.flag()?;
                let next_va = r.word()?;
                let root_vmar = r.index()?;
                let heap_vmar = r.index()?;
                if quarantined && !exited {
                    return Err(bad);
                }
                let mut mappings = Vec::new();
                for _ in 0..r.count(65536)? {
                    mappings.push(Mapping {
                        vmo: r.index()?,
                        vmar: r.index()?,
                        offset: r.word()?,
                        va: r.word()?,
                        size: r.word()?,
                        rights: u32::try_from(r.word()?).map_err(|_| bad)?,
                    });
                }
                put(
                    &mut self.processes,
                    index,
                    Process {
                        name,
                        package,
                        hardware: hardware as u32,
                        resource_group_id,
                        realtime_scheduling,
                        authority: authority as u32,
                        quarantined: false,
                        root,
                        asid,
                        userspace_pac_key,
                        context,
                        running,
                        exited,
                        mappings,
                        root_vmar,
                        heap_vmar,
                        next_va,
                    },
                    32,
                )?;
            }
            VMO => {
                let vmo = if r.flag()? {
                    Some(Vmo {
                        size: r.word()?,
                        refs: r.index()?,
                        device: r.flag()?,
                        bootfs: r.flag()?,
                        backing: read_vmo_backing(&mut r)?,
                    })
                } else {
                    None
                };
                put(&mut self.vmos, index, vmo, 262144)?;
                self.reclaim_cursor = self.reclaim_cursor.min(index);
            }
            HANDLE => {
                let cap = if r.flag()? {
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
                        _ => return Err(bad),
                    };
                    Some(Capability {
                        object,
                        rights: u32::try_from(r.word()?).map_err(|_| bad)?,
                        owner: r.index()?,
                    })
                } else {
                    None
                };
                put(&mut self.handles, index, cap, 262144)?;
            }
            CHANNEL => {
                let mut c = Channel {
                    identity: 0,
                    queues: [VecDeque::new(), VecDeque::new()],
                    calls: [VecDeque::new(), VecDeque::new()],
                    refs: [0, 0],
                    next_call_id: r.word()?,
                    policy: ChannelPolicy {
                        enable_priority_inheritance: r.flag()?,
                        enable_timeslice_donation: r.flag()?,
                    },
                };
                for end in 0..2 {
                    c.refs[end] = r.index()?;
                    for _ in 0..r.count(64)? {
                        c.queues[end].push_back(read_runtime_message(&mut r)?);
                    }
                    for _ in 0..r.count(64)? {
                        let id = r.word()?;
                        let caller_end = r.index()?;
                        if caller_end > 1 {
                            return Err(bad);
                        }
                        let caller_thread = r.index()?;
                        let serving = r.flag()?;
                        let server_thread = match r.word()? {
                            value if value == usize::MAX as u64 => None,
                            value => Some(usize::try_from(value).map_err(|_| bad)?),
                        };
                        let request = read_runtime_message(&mut r)?;
                        let reply = if r.flag()? {
                            Some(read_runtime_message(&mut r)?)
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
                // Older channel records derive identity from their append-only slot.
                c.identity = if r.finished() {
                    index as u64 * 2 + 1
                } else {
                    r.word()?
                };
                if c.identity == 0 || c.identity % 2 != 1 || c.identity == u64::MAX {
                    return Err(bad);
                }
                put(&mut self.channels, index, c, 65536)?;
            }
            SOCKET => {
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
                        let n = r.count(65536)?;
                        s.queues[end].push_back(r.bytes(n)?.to_vec());
                    }
                }
                put(&mut self.sockets, index, s, 65536)?;
            }
            PIN => {
                let pin = if r.flag()? {
                    Some((r.index()?, r.index()?))
                } else {
                    None
                };
                put(&mut self.pins, index, pin, 262144)?;
            }
            THREAD => {
                let thread = Thread {
                    process: r.index()?,
                    context: read_saved_context(&mut r)?,
                    running: r.flag()?,
                    exited: r.flag()?,
                    blocked_futex: match r.word()? {
                        0 => None,
                        value => Some(value),
                    },
                    blocked_wait_many: r.flag().unwrap_or(false),
                    exit_code: r.word()? as i32,
                };
                put(&mut self.threads, index, thread, MAX_THREADS)?;
            }
            VMAR => {
                let vmar = if r.flag()? {
                    let owner = r.index()?;
                    let raw_parent = r.word()?;
                    Some(Vmar {
                        owner,
                        parent: if raw_parent == usize::MAX as u64 {
                            None
                        } else {
                            Some(usize::try_from(raw_parent).map_err(|_| bad)?)
                        },
                        base: r.word()?,
                        size: r.word()?,
                        rights: u32::try_from(r.word()?).map_err(|_| bad)?,
                        refs: r.index()?,
                        destroyed: r.flag()?,
                    })
                } else {
                    None
                };
                put(&mut self.vmars, index, vmar, 262144)?;
            }
            PROFILE => {
                let profile = read_profile(&mut r)?;
                put(&mut self.profiles, index, profile, 262144)?;
            }
            IOMMU_DOMAIN => {
                let domain = dma_snapshot::read_domain(&mut r)?;
                put(&mut self.iommu_domains, index, domain, 262144)?;
            }
            DMA_MAPPING => {
                let mapping = dma_snapshot::read_mapping(&mut r)?;
                put(&mut self.dma_mappings, index, mapping, 262144)?;
            }
            _ => return Err(bad),
        }
        if !r.finished() {
            return Err(bad);
        }
        Ok(())
    }
    pub fn validate_live_snapshot(&self) -> CodecResult<()> {
        self.validate_snapshot()
    }
}

fn put<T>(slots: &mut Vec<T>, index: usize, value: T, limit: usize) -> CodecResult<()> {
    if index > slots.len() || index >= limit {
        return Err(TransplantError::InvalidRuntimeSnapshot);
    }
    if index == slots.len() {
        slots.push(value);
    } else {
        slots[index] = value;
    }
    Ok(())
}

fn write_vmo_backing(w: &mut Writer<'_>, backing: &VmoBacking) -> CodecResult<()> {
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

fn read_vmo_backing(r: &mut Reader<'_>) -> CodecResult<VmoBacking> {
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
