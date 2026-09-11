//! Bounded scrubbing of memory detached by an atomic process retirement.
use super::*;
use bexos_boot::PAGE;

impl<B: Backend> Runtime<B> {
    pub fn has_pending_reclamation(&self) -> bool {
        self.vmos[self.reclaim_cursor.min(self.vmos.len())..]
            .iter()
            .flatten()
            .any(|v| v.refs == 0)
    }

    /// Leave pages owned by the kernel until maintenance has scrubbed them.
    /// A zero-ref lazy VMO has no accessible handles, mappings, pins, or DMA
    /// registrations; it is the checkpointed reclamation record itself.
    pub(super) fn defer_vmo_reclamation(&mut self, id: usize) {
        let vmo = self.vmos[id].as_mut().unwrap();
        debug_assert_eq!(vmo.refs, 0);
        debug_assert!(!vmo.device);
        if let VmoBacking::Contiguous { base } = vmo.backing {
            vmo.backing = VmoBacking::LazyAnonymous {
                pages: (0..vmo.size / PAGE)
                    .map(|i| Some(base + i * PAGE))
                    .collect(),
            };
        }
        self.reclaim_cursor = self.reclaim_cursor.min(id);
        self.changed(VMO, id);
    }

    /// Perform at most `page_budget` page scrubs. Pages are released only after
    /// zeroing, and pending VMOs remain part of full and incremental snapshots.
    /// Skip the quiesced handover window so an earlier retirement cannot consume
    /// the current transaction's cutover budget.
    pub fn reclaim_retired_pages(&mut self, page_budget: usize) -> usize {
        use bexos_migration::Phase;
        if self
            .handover
            .as_ref()
            .is_some_and(|h| matches!(h.session.phase(), Phase::Quiesced | Phase::Ready))
        {
            return 0;
        }
        let mut released = 0;
        while released < page_budget && self.reclaim_cursor < self.vmos.len() {
            let id = self.reclaim_cursor;
            let Some(vmo) = self.vmos[id].as_mut().filter(|v| v.refs == 0) else {
                self.reclaim_cursor += 1;
                continue;
            };
            let VmoBacking::LazyAnonymous { pages } = &mut vmo.backing else {
                unreachable!("retired VMO must retain a page ownership list");
            };
            let Some(index) = pages.iter().rposition(Option::is_some) else {
                self.vmos[id] = None;
                self.changed(VMO, id);
                self.reclaim_cursor += 1;
                continue;
            };
            let base = pages[index].unwrap();
            pages.truncate(index);
            vmo.size = pages.len() as u64 * PAGE;
            let finished = pages.is_empty();
            if vmo.bootfs {
                self.bootfs_pages -= 1;
                self.reclaimed_pages += 1;
            }
            self.backend.zero(base, PAGE);
            self.backend.release(base, 1);
            released += 1;
            if finished {
                self.vmos[id] = None;
                self.reclaim_cursor += 1;
            }
            self.changed(VMO, id);
        }
        released
    }
}
