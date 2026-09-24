//! Atomic ownership transfer for independently prepared userspace replacements.
use super::*;
use bexos_migration::{Phase, Session, Timeouts};

pub const AUTH_APP_MANAGER: u32 = 1;
pub const AUTH_PLATFORM_UPDATE: u32 = 2;
pub const AUTH_SECURE_MONITOR: u32 = 4;
pub const AUTH_VALID_MASK: u32 = AUTH_APP_MANAGER | AUTH_PLATFORM_UPDATE | AUTH_SECURE_MONITOR;

pub struct Handover {
    pub coordinator: usize,
    pub source: usize,
    pub target: usize,
    pub session: Session,
    pub handles: Vec<u64>,
    pub mappings: Vec<Mapping>,
    pub pins: Vec<u64>,
    source_running: bool,
}

impl<B: Backend> Runtime<B> {
    fn preserved_pin_resource(&self, token: u64) -> Result<(usize, usize, Option<usize>)> {
        if token & memory::DMA_TOKEN_TAG != 0 {
            let index = memory::dma_token_index(token)?;
            self.dma_mappings
                .get(index)
                .and_then(|mapping| *mapping)
                .map(|mapping| (mapping.owner, mapping.vmo, Some(mapping.domain)))
                .ok_or(Status::ErrInvalidHandle)
        } else {
            let index = token.checked_sub(1).ok_or(Status::ErrInvalidHandle)? as usize;
            self.pins
                .get(index)
                .and_then(|pin| *pin)
                .map(|(owner, vmo)| (owner, vmo, None))
                .ok_or(Status::ErrInvalidHandle)
        }
    }

    pub fn has_authority(&self, authority: u32) -> bool {
        self.processes
            .get(self.current)
            .is_some_and(|p| !p.quarantined && p.authority & authority == authority)
    }

    pub fn set_process_authority(&mut self, process: u64, authority: u32) -> Result<()> {
        if !self.has_authority(AUTH_APP_MANAGER) || authority & !AUTH_VALID_MASK != 0 {
            return Err(Status::ErrAccessDenied);
        }
        let Object::Process(id) = self.capability(process, ADMIN)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if self.processes[id].quarantined || self.processes[id].exited {
            return Err(Status::ErrInvalidArgs);
        }
        self.processes[id].authority = authority;
        self.changed(PROCESS, id);
        Ok(())
    }

    pub fn discard_candidate(&mut self, process: u64) -> Result<()> {
        if !self.has_authority(AUTH_APP_MANAGER) {
            return Err(Status::ErrAccessDenied);
        }
        let Object::Process(id) = self.capability(process, ADMIN)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if id == self.current
            || self.processes[id].running
            || self
                .handover
                .as_ref()
                .is_some_and(|h| id == h.source || id == h.target)
        {
            return Err(Status::ErrAccessDenied);
        }
        self.retire_process(id);
        Ok(())
    }

    pub fn begin_handover(
        &mut self,
        source: u64,
        target: u64,
        generation: u64,
        now: u64,
        timeouts: Timeouts,
    ) -> Result<()> {
        if !self.has_authority(AUTH_APP_MANAGER) {
            return Err(Status::ErrAccessDenied);
        }
        if self.handover.is_some() || self.dirty.is_some() {
            return Err(Status::ErrAlreadyExists);
        }
        let source = if source == 0 {
            self.current
        } else {
            match self.capability(source, ADMIN)?.object {
                Object::Process(id) => id,
                _ => return Err(Status::ErrInvalidHandle),
            }
        };
        let Object::Process(target) = self.capability(target, ADMIN)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if source == target
            || self.processes[source].exited
            || !self.processes[source].running
            || self.processes[target].exited
            || self.processes[target].running
            || self.processes[target].authority != 0
            || self.processes[source].package != self.processes[target].package
            || self.processes[source].hardware != self.processes[target].hardware
        {
            return Err(Status::ErrInvalidArgs);
        }
        let session =
            Session::new(generation, now, timeouts).map_err(|_| Status::ErrInvalidArgs)?;
        self.processes[target].quarantined = true;
        self.handover = Some(Handover {
            coordinator: self.current,
            source,
            target,
            session,
            handles: Vec::new(),
            mappings: Vec::new(),
            pins: Vec::new(),
            source_running: true,
        });
        Ok(())
    }

    fn handover_actor(&self) -> Result<&Handover> {
        let h = self.handover.as_ref().ok_or(Status::ErrInvalidArgs)?;
        if self.current != h.coordinator && self.current != h.source {
            return Err(Status::ErrAccessDenied);
        }
        Ok(h)
    }

    /// Only the source or its app-manager coordinator can describe resources.
    /// Numerical handles are descriptors, not borrowed authority for the caller.
    pub fn preserve_handle(&mut self, handle: u64) -> Result<()> {
        let h = self.handover_actor()?;
        if !matches!(
            h.session.phase(),
            Phase::Staged | Phase::Bulk | Phase::CatchUp | Phase::Quiesced
        ) {
            return Err(Status::ErrInvalidArgs);
        }
        let cap = self
            .handles
            .get(handle.checked_sub(1).ok_or(Status::ErrInvalidHandle)? as usize)
            .and_then(|c| *c)
            .ok_or(Status::ErrInvalidHandle)?;
        if cap.owner != h.source
            || cap.rights & TRANSFER == 0
                && !matches!(
                    cap.object,
                    Object::Process(_) | Object::Space(_) | Object::Thread(_)
                )
        {
            return Err(Status::ErrAccessDenied);
        }
        // Source-owned descriptors for the candidate's process/space/thread
        // become its management descriptors after appd replaces itself. The
        // source's own descriptors cannot outlive retirement.
        if matches!(cap.object, Object::Process(id) | Object::Space(id) | Object::Thread(id) if id == h.source)
        {
            return Err(Status::ErrAccessDenied);
        }
        let h = self.handover.as_mut().unwrap();
        if !h.handles.contains(&handle) {
            h.handles.push(handle);
        }
        Ok(())
    }

    pub fn preserve_mapping(
        &mut self,
        handle: u64,
        offset: u64,
        va: u64,
        size: u64,
        rights: u32,
    ) -> Result<()> {
        let h = self.handover_actor()?;
        let cap = self
            .handles
            .get(handle.checked_sub(1).ok_or(Status::ErrInvalidHandle)? as usize)
            .and_then(|c| *c)
            .ok_or(Status::ErrInvalidHandle)?;
        if cap.owner != h.source {
            return Err(Status::ErrAccessDenied);
        }
        let Object::Vmo(vmo) = cap.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if cap.rights & (MAP | rights) != MAP | rights
            || rights & !(READ | WRITE) != 0
            || rights & READ == 0
        {
            return Err(Status::ErrAccessDenied);
        }
        let backing = self.vmos[vmo].as_ref().ok_or(Status::ErrInvalidHandle)?;
        let target_root_vmar = self.processes[h.target].root_vmar;
        if size == 0
            || size % 4096 != 0
            || offset % 4096 != 0
            || va % 4096 != 0
            || offset
                .checked_add(size)
                .is_none_or(|end| end > backing.size)
            || va < bexos_boot::USER_START
            || va
                .checked_add(size)
                .is_none_or(|end| end > bexos_boot::USER_END)
            || self.processes[h.target]
                .mappings
                .iter()
                .chain(h.mappings.iter())
                .any(|m| va < m.va + m.size && m.va < va + size)
        {
            return Err(Status::ErrInvalidArgs);
        }
        self.preserve_handle(handle)?;
        self.handover.as_mut().unwrap().mappings.push(Mapping {
            vmo,
            vmar: target_root_vmar,
            offset,
            va,
            size,
            rights,
        });
        Ok(())
    }

    pub fn preserve_pin(&mut self, token: u64) -> Result<()> {
        let h = self.handover_actor()?;
        if !matches!(
            h.session.phase(),
            Phase::Staged | Phase::Bulk | Phase::CatchUp | Phase::Quiesced
        ) {
            return Err(Status::ErrInvalidArgs);
        }
        let (owner, _, _) = self.preserved_pin_resource(token)?;
        if owner != h.source {
            return Err(Status::ErrAccessDenied);
        }
        let h = self.handover.as_mut().unwrap();
        if !h.pins.contains(&token) {
            h.pins.push(token);
        }
        Ok(())
    }

    pub fn handover_bulk(&mut self, now: u64) -> Result<()> {
        self.handover_actor()?;
        self.handover
            .as_mut()
            .unwrap()
            .session
            .begin_bulk(now)
            .map_err(|_| Status::ErrInvalidArgs)
    }
    pub fn handover_catch_up(&mut self, now: u64) -> Result<()> {
        self.handover_actor()?;
        self.handover
            .as_mut()
            .unwrap()
            .session
            .bulk_complete(now)
            .map_err(|_| Status::ErrInvalidArgs)
    }
    pub fn begin_cutover_handover(&mut self, sequence: u64, now: u64) -> Result<()> {
        let h = self.handover_actor()?;
        if self.current != h.source {
            return Err(Status::ErrAccessDenied);
        }
        self.handover
            .as_mut()
            .unwrap()
            .session
            .quiesce(sequence, now)
            .map_err(|_| Status::ErrInvalidArgs)
    }
    pub fn quiesce_handover(&mut self, sequence: u64, now: u64) -> Result<()> {
        let h = self.handover_actor()?;
        if self.current != h.source {
            return Err(Status::ErrAccessDenied);
        }
        let h = self.handover.as_mut().unwrap();
        if h.session.phase() == Phase::CatchUp {
            h.session
                .quiesce(sequence, now)
                .map_err(|_| Status::ErrInvalidArgs)?;
        } else {
            h.session.poll(now).map_err(|_| Status::ErrTimedOut)?;
            if h.session.phase() != Phase::Quiesced || h.session.final_sequence() != Some(sequence)
            {
                return Err(Status::ErrInvalidArgs);
            }
        }
        self.processes[h.source].running = false;
        self.scheduler.quiesce_process(h.source as u64 + 1);
        self.changed(PROCESS, self.current);
        Ok(())
    }
    pub fn ready_handover(&mut self, sequence: u64, now: u64) -> Result<()> {
        let h = self.handover.as_mut().ok_or(Status::ErrInvalidArgs)?;
        if self.current != h.target {
            return Err(Status::ErrAccessDenied);
        }
        if self.processes[h.source].running {
            return Err(Status::ErrInvalidArgs);
        }
        h.session
            .ready(sequence, now)
            .map_err(|_| Status::ErrInvalidArgs)
    }

    pub fn commit_handover(&mut self, now: u64) -> Result<()> {
        let h = self.handover.as_ref().ok_or(Status::ErrInvalidArgs)?;
        if self.current != h.target && self.current != h.coordinator {
            return Err(Status::ErrAccessDenied);
        }
        if self.current == h.source || h.session.phase() != Phase::Ready {
            return Err(Status::ErrInvalidArgs);
        }
        // Validate every resource before mutating ownership. Handles may have
        // been closed or transferred while the bulk copy was in progress.
        for handle in &h.handles {
            if self
                .handles
                .get(*handle as usize - 1)
                .and_then(|c| *c)
                .is_none_or(|c| c.owner != h.source)
            {
                return Err(Status::ErrInvalidHandle);
            }
        }
        for token in &h.pins {
            if self.preserved_pin_resource(*token)?.0 != h.source {
                return Err(Status::ErrInvalidHandle);
            }
        }
        // A pin must remain backed by an explicitly preserved VMO, even if the
        // old driver has released its mapping during quiescence.
        for token in &h.pins {
            let (_, vmo, domain) = self.preserved_pin_resource(*token)?;
            if !h.handles.iter().any(|handle| {
                self.handles[*handle as usize - 1].unwrap().object == Object::Vmo(vmo)
            }) {
                return Err(Status::ErrInvalidArgs);
            }
            if let Some(domain) = domain {
                if !h.handles.iter().any(|handle| {
                    self.handles[*handle as usize - 1].unwrap().object
                        == Object::IommuDomain(domain)
                }) {
                    return Err(Status::ErrInvalidArgs);
                }
            }
        }
        let target = h.target;
        let source = h.source;
        let root = self.processes[target].root;
        let mappings = h.mappings.clone();
        if mappings.iter().any(|m| {
            self.processes[target]
                .mappings
                .iter()
                .any(|other| m.va < other.va + other.size && other.va < m.va + m.size)
        }) {
            return Err(Status::ErrInvalidArgs);
        }
        let mut mapped = Vec::new();
        for m in &mappings {
            let device = self.vmos[m.vmo]
                .as_ref()
                .ok_or(Status::ErrInvalidHandle)?
                .device_mapping();
            for off in (0..m.size).step_by(4096) {
                let (pa, zero_page) = self.vmo_page_phys(m.vmo, m.offset + off, false)?;
                let rights = if zero_page {
                    m.rights & !WRITE
                } else {
                    m.rights
                };
                if let Err(error) = self.backend.map_page(root, m.va + off, pa, rights, device) {
                    for va in mapped {
                        self.backend.unmap_page(root, va);
                    }
                    self.backend.flush_mappings();
                    return Err(error);
                }
                mapped.push(m.va + off);
            }
        }
        self.backend.flush_mappings();
        let now = self.backend.monotonic_ms().unwrap_or(now);
        if let Err(_) = self.handover.as_mut().unwrap().session.commit(now) {
            for va in mapped {
                self.backend.unmap_page(root, va);
            }
            self.backend.flush_mappings();
            self.abort_handover_internal();
            return Err(Status::ErrTimedOut);
        }
        let h = self.handover.take().unwrap();
        for handle in h.handles {
            self.handles[handle as usize - 1].as_mut().unwrap().owner = target;
        }
        for token in h.pins {
            if token & memory::DMA_TOKEN_TAG != 0 {
                let index = memory::dma_token_index(token).unwrap();
                self.dma_mappings[index].as_mut().unwrap().owner = target;
                self.changed(incremental::DMA_MAPPING, index);
            } else {
                self.pins[token as usize - 1].as_mut().unwrap().0 = target;
            }
        }
        for m in mappings {
            self.vmos[m.vmo].as_mut().unwrap().refs += 1;
            self.processes[target].next_va = self.processes[target].next_va.max(m.va + m.size);
            self.processes[target].mappings.push(m);
        }
        self.processes[target].authority = self.processes[source].authority;
        self.processes[source].authority = 0;
        self.processes[target].quarantined = false;
        self.retire_process(source);
        Ok(())
    }
    pub fn abort_handover(&mut self) -> Result<()> {
        let h = self.handover.as_ref().ok_or(Status::ErrInvalidArgs)?;
        if self.current != h.coordinator && self.current != h.source && self.current != h.target {
            return Err(Status::ErrAccessDenied);
        }
        self.abort_handover_internal();
        Ok(())
    }
    pub fn poll_handover(&mut self, now: u64) {
        if self
            .handover
            .as_mut()
            .is_some_and(|h| h.session.poll(now).is_err())
        {
            self.abort_handover_internal();
        }
    }
    fn abort_handover_internal(&mut self) {
        if let Some(h) = self.handover.take() {
            self.processes[h.source].running = h.source_running && !self.processes[h.source].exited;
            if self.processes[h.source].running {
                self.scheduler.resume_quiesced_process(h.source as u64 + 1);
            }
            self.retire_process(h.target);
        }
    }
    pub(super) fn retire_process(&mut self, id: usize) {
        let started = self.backend.monotonic_ms().unwrap_or(0);
        let current = self.current;
        self.current = id;
        let previous_defer = core::mem::replace(&mut self.defer_reclamation, true);
        self.processes[id].exited = true;
        self.processes[id].running = false;
        self.processes[id].authority = 0;
        self.processes[id].quarantined = false;
        self.changed(PROCESS, id);
        for thread_id in 0..self.threads.len() {
            if self.threads[thread_id].process == id {
                let restricted_vmo = {
                    let thread = &mut self.threads[thread_id];
                    thread.running = false;
                    thread.exited = true;
                    thread.blocked_futex = None;
                    thread.blocked_wait_many = false;
                    thread.restricted.take().map(|binding| binding.state_vmo)
                };
                if let Some(vmo) = restricted_vmo {
                    self.release_vmo(vmo);
                }
                // Retiring the address space must also remove every thread
                // from scheduler ownership before its mappings are released.
                let _ = self.scheduler.exit_task(thread_id as u64 + 1, 0);
                self.changed(THREAD, thread_id);
            }
        }
        let handles: Vec<_> = self
            .handles
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.filter(|c| c.owner == id).map(|_| i as u64 + 1))
            .collect();
        for h in handles {
            let _ = self.close(h);
        }
        let handles_done = self.backend.monotonic_ms().unwrap_or(started);
        let maps = core::mem::take(&mut self.processes[id].mappings);
        for map in maps {
            self.release_vmo(map.vmo);
        }
        let mappings_done = self.backend.monotonic_ms().unwrap_or(handles_done);
        for index in 0..self.dma_mappings.len() {
            if self.dma_mappings[index].is_some_and(|mapping| mapping.owner == id) {
                let mapping = self.dma_mappings[index].take().unwrap();
                self.changed(incremental::DMA_MAPPING, index);
                self.release_vmo(mapping.vmo);
            }
        }
        for i in 0..self.pins.len() {
            if let Some((owner, vmo)) = self.pins[i] {
                if owner == id {
                    self.backend.revoke_shared_pin(i as u64 + 1);
                    self.pins[i] = None;
                    self.changed(PIN, i);
                    self.release_vmo(vmo);
                }
            }
        }
        // Page-table reclamation is performed only once and does not touch
        // other processes' mappings of preserved VMOs.
        let resources_done = self.backend.monotonic_ms().unwrap_or(mappings_done);
        let root = core::mem::replace(&mut self.processes[id].root, 0);
        if root != 0 {
            self.backend.destroy_space(root);
        }
        let tables_done = self.backend.monotonic_ms().unwrap_or(resources_done);
        self.backend.trace_retirement(
            id,
            [
                handles_done.saturating_sub(started),
                mappings_done.saturating_sub(handles_done),
                resources_done.saturating_sub(mappings_done),
                tables_done.saturating_sub(resources_done),
            ],
        );
        self.current = current;
        self.defer_reclamation = previous_defer;
        self.wake_waiters_for_object(
            Object::Process(id),
            crate::kernel_services::SIGNAL_TERMINATED,
        );
    }
}
