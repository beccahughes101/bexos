use alloc::boxed::Box;
pub(crate) mod reclamation;
use bexos_allocator::Heap;
use bexos_boot::{BootHandoff, PAGE, RAM_END, RAM_START};
pub const HEAP_BYTES: usize = 8 * 1024 * 1024;
#[global_allocator]
static HEAP: Heap = Heap::new();
unsafe extern "C" {
    static __kernel_end: u8;
}
pub fn kernel_end() -> u64 {
    core::ptr::addr_of!(__kernel_end) as u64
}
pub fn init_heap() -> u64 {
    let start = kernel_end();
    let end = start + HEAP_BYTES as u64;
    unsafe {
        HEAP.init(start as usize, HEAP_BYTES);
    }
    crate::log_line("kernel: allocator initialized");
    end
}
pub fn init(handoff: &BootHandoff) -> u64 {
    let end = init_heap();
    assert!(end <= handoff.bootfs_addr);
    end
}
const FRAME_COUNT: usize = ((RAM_END - RAM_START) / PAGE) as usize;
const WORD_BITS: usize = 64;
const FRAME_WORDS: usize = FRAME_COUNT / WORD_BITS;

/// A compact physical-frame bitmap.  Keeping this in the kernel bootstrap
/// stack is temporary, so it must remain small enough for an exception stack.
pub struct Frames {
    used: Box<[u64; FRAME_WORDS]>,
    was_bootfs: Box<[u64; FRAME_WORDS]>,
    reused: u64,
    allocation_hint: usize,
    dirty: Option<bexos_migration::dirty::DirtySet>,
}
impl Frames {
    pub const SNAPSHOT_WORDS: usize = FRAME_WORDS * 2 + 1;
    pub fn empty() -> Self {
        Self {
            used: Box::new([u64::MAX; FRAME_WORDS]),
            was_bootfs: Box::new([0; FRAME_WORDS]),
            reused: 0,
            allocation_hint: 0,
            dirty: None,
        }
    }
    pub fn begin_live_snapshot(&mut self) {
        self.dirty = Some(bexos_migration::dirty::DirtySet::new(Self::SNAPSHOT_WORDS));
    }
    pub fn end_live_snapshot(&mut self) {
        self.dirty = None;
    }
    pub fn dirty_next(&self) -> Option<usize> {
        self.dirty
            .as_ref()
            .and_then(|d| d.next().ok().flatten())
            .map(|n| n as usize)
    }
    pub fn word(&self, index: usize) -> Option<u64> {
        match index {
            i if i < FRAME_WORDS => Some(self.used[i]),
            i if i < FRAME_WORDS * 2 => Some(self.was_bootfs[i - FRAME_WORDS]),
            i if i == FRAME_WORDS * 2 => Some(self.reused),
            _ => None,
        }
    }
    pub fn adopt_word(&mut self, index: usize, value: u64) -> Result<(), ()> {
        match index {
            i if i < FRAME_WORDS => self.used[i] = value,
            i if i < FRAME_WORDS * 2 => self.was_bootfs[i - FRAME_WORDS] = value,
            i if i == FRAME_WORDS * 2 => self.reused = value,
            _ => return Err(()),
        }
        Ok(())
    }
    pub fn copied(&mut self, index: usize) {
        if let Some(d) = &mut self.dirty {
            d.copied(index as u64);
        }
    }
    pub fn write_snapshot(
        &self,
        w: &mut bexos_kernel_core::transplant::codec::Writer<'_>,
    ) -> Result<(), bexos_kernel_core::transplant::TransplantError> {
        w.word(FRAME_WORDS as u64)?;
        for &n in self.used.iter() {
            w.word(n)?;
        }
        for &n in self.was_bootfs.iter() {
            w.word(n)?;
        }
        w.word(self.reused)
    }
    pub fn read_snapshot(
        r: &mut bexos_kernel_core::transplant::codec::Reader<'_>,
    ) -> Result<Self, bexos_kernel_core::transplant::TransplantError> {
        if r.word()? != FRAME_WORDS as u64 {
            return Err(bexos_kernel_core::transplant::TransplantError::UnsupportedVersion);
        }
        let mut frames = Self {
            used: Box::new([0; FRAME_WORDS]),
            was_bootfs: Box::new([0; FRAME_WORDS]),
            reused: 0,
            allocation_hint: 0,
            dirty: None,
        };
        for n in &mut frames.used {
            *n = r.word()?;
        }
        for n in &mut frames.was_bootfs {
            *n = r.word()?;
        }
        frames.reused = r.word()?;
        Ok(frames)
    }
    pub fn new(first: u64, handoff: &BootHandoff) -> Self {
        crate::log_line("kernel: frame bitmap allocation begin");
        let mut f = Self {
            used: Box::new([u64::MAX; FRAME_WORDS]),
            was_bootfs: Box::new([0; FRAME_WORDS]),
            reused: 0,
            allocation_hint: 0,
            dirty: None,
        };
        crate::log_line("kernel: frame bitmap storage initialized");
        let alloc_start = page_index_at_or_after(first);
        let alloc_end = page_index_at_or_after(handoff.ram_end);
        set_bit_range(&mut f.used, alloc_start, alloc_end, false);
        crate::log_line("kernel: frame bitmap free range initialized");
        if handoff.framebuffer.valid() {
            let start = page_index_at_or_after(handoff.framebuffer.address).max(alloc_start);
            let end =
                page_index_at_or_after(handoff.framebuffer.address + handoff.framebuffer.length)
                    .min(alloc_end);
            if start < end {
                set_bit_range(&mut f.used, start, end, true);
            }
        }

        let bootfs_start = page_index_at_or_after(handoff.bootfs_addr);
        let bootfs_end = page_index_at_or_after(handoff.bootfs_addr + handoff.bootfs_len);
        set_bit_range(
            &mut f.used,
            bootfs_start.max(alloc_start),
            bootfs_end.min(alloc_end),
            true,
        );
        set_bit_range(&mut f.was_bootfs, bootfs_start, bootfs_end, true);
        crate::log_line("kernel: frame bitmap BootFS range reserved");

        // The verified boot-evidence page is imported into appd as an owned
        // VMO. Reserve it before ordinary allocation so retiring appd can
        // release that single owner without colliding with a page table or
        // anonymous VMO that happened to reuse the same physical page.
        let evidence_start = page_index_at_or_after(handoff.boot_evidence_addr);
        let evidence_end = page_index_at_or_after(
            handoff.boot_evidence_addr + handoff.boot_evidence_len.div_ceil(PAGE) * PAGE,
        );
        set_bit_range(
            &mut f.used,
            evidence_start.max(alloc_start),
            evidence_end.min(alloc_end),
            true,
        );
        crate::log_line("kernel: frame bitmap boot evidence reserved");

        let update_start = page_index_at_or_after(handoff.update_base);
        let update_end = page_index_at_or_after(handoff.update_base + handoff.update_len);
        set_bit_range(
            &mut f.used,
            update_start.max(alloc_start),
            update_end.min(alloc_end),
            true,
        );
        crate::log_line("kernel: frame bitmap update range reserved");
        f
    }
    fn used(&self, index: usize) -> bool {
        self.used[index / WORD_BITS] & (1 << (index % WORD_BITS)) != 0
    }
    fn set_used(&mut self, index: usize, value: bool) {
        if let Some(d) = &mut self.dirty {
            d.mark((index / WORD_BITS) as u64);
        }
        let word = &mut self.used[index / WORD_BITS];
        let mask = 1 << (index % WORD_BITS);
        if value { *word |= mask } else { *word &= !mask }
    }
    fn was_bootfs(&self, index: usize) -> bool {
        self.was_bootfs[index / WORD_BITS] & (1 << (index % WORD_BITS)) != 0
    }
    fn set_was_bootfs(&mut self, index: usize, value: bool) {
        if let Some(d) = &mut self.dirty {
            d.mark((FRAME_WORDS + index / WORD_BITS) as u64);
        }
        let word = &mut self.was_bootfs[index / WORD_BITS];
        let mask = 1 << (index % WORD_BITS);
        if value { *word |= mask } else { *word &= !mask }
    }
    pub fn allocate(&mut self, pages: u64) -> Option<u64> {
        let pages = usize::try_from(pages).ok()?;
        if pages == 0 || pages > FRAME_COUNT {
            return None;
        }
        let hint = self.allocation_hint.min(FRAME_COUNT);
        let first = self
            .find_free_run(hint, FRAME_COUNT, pages)
            .or_else(|| self.find_free_run(0, hint, pages))?;
        for j in first..first + pages {
            self.set_used(j, true);
            if self.was_bootfs(j) {
                self.reused += 1;
                if let Some(d) = &mut self.dirty {
                    d.mark((FRAME_WORDS * 2) as u64);
                }
                self.set_was_bootfs(j, false);
            }
        }
        self.allocation_hint = (first + pages) % FRAME_COUNT;
        let base = RAM_START + first as u64 * PAGE;
        unsafe {
            core::ptr::write_bytes(base as *mut u8, 0, pages * PAGE as usize);
        }
        Some(base)
    }
    fn find_free_run(&self, start: usize, end: usize, pages: usize) -> Option<usize> {
        let mut run = 0;
        for i in start..end {
            if self.used(i) {
                run = 0;
            } else {
                run += 1;
                if run == pages {
                    return Some(i + 1 - pages);
                }
            }
        }
        None
    }
    pub fn release(&mut self, base: u64, pages: u64) {
        assert!(base % PAGE == 0 && base >= RAM_START && base + pages * PAGE <= RAM_END);
        let first = ((base - RAM_START) / PAGE) as usize;
        for i in first..first + pages as usize {
            assert!(
                self.used(i),
                "double release of physical frame base={base:#x} pages={pages} index={i}"
            );
            self.set_used(i, false);
        }
        self.allocation_hint = self.allocation_hint.min(first);
    }
    pub fn free_pages(&self) -> u64 {
        self.used.iter().map(|word| word.count_zeros() as u64).sum()
    }
    pub fn reused(&self) -> u64 {
        self.reused
    }
}

fn page_index_at_or_after(address: u64) -> usize {
    if address <= RAM_START {
        return 0;
    }
    usize::try_from((address - RAM_START).div_ceil(PAGE))
        .unwrap_or(FRAME_COUNT)
        .min(FRAME_COUNT)
}

fn set_bit_range(words: &mut [u64; FRAME_WORDS], start: usize, end: usize, value: bool) {
    if start >= end {
        return;
    }
    let first_word = start / WORD_BITS;
    let last_word = (end - 1) / WORD_BITS;
    for (word_index, word) in words
        .iter_mut()
        .enumerate()
        .take(last_word + 1)
        .skip(first_word)
    {
        let word_start = word_index * WORD_BITS;
        let bit_start = start.saturating_sub(word_start).min(WORD_BITS);
        let bit_end = end.saturating_sub(word_start).min(WORD_BITS);
        let low = if bit_start == 0 {
            u64::MAX
        } else {
            u64::MAX << bit_start
        };
        let high = if bit_end == WORD_BITS {
            u64::MAX
        } else {
            (1u64 << bit_end) - 1
        };
        let mask = low & high;
        if value {
            *word |= mask;
        } else {
            *word &= !mask;
        }
    }
}
