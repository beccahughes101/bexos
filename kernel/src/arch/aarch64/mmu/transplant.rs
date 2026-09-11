//! Executable mappings for authenticated replacement-kernel preparation.
use super::*;
use alloc::vec::Vec;
use bexos_kernel_core::{loader::LoadPlan, runtime::Runtime, transplant::KernelRange};

static STAGED_CODE: crate::state::Global<Vec<KernelRange>> = crate::state::Global::new(Vec::new());

fn map_ranges(
    backend: &mut PhysicalBackend,
    root: u64,
    ranges: &[KernelRange],
    executable: bool,
) -> Result<()> {
    for range in ranges {
        let end = align_up(range.start + range.len);
        for pa in (align_down(range.start)..end).step_by(4096) {
            backend.map_kernel_identity_page(
                root,
                pa,
                if executable {
                    Access::KernelReadOnly
                } else {
                    Access::KernelReadWrite
                },
                executable,
            )?;
        }
    }
    Ok(())
}

pub fn install_staged_code(backend: &mut PhysicalBackend, root: u64) -> Result<()> {
    STAGED_CODE.with(|ranges| map_ranges(backend, root, ranges, true))
}

pub fn stage(rt: &mut Runtime<PhysicalBackend>, plan: &LoadPlan) -> Result<()> {
    let ranges: Vec<_> = plan
        .segments
        .iter()
        .filter(|segment| segment.rights & 8 != 0)
        .map(|segment| KernelRange::new(segment.vaddr, segment.file_size))
        .collect();
    // Every process root has a kernel-only identity map. Preparation runs
    // between EL0 dispatches and may therefore enter through any such root.
    let result = rt
        .processes
        .iter()
        .filter(|process| process.root != 0)
        .try_for_each(|process| map_ranges(&mut rt.backend, process.root, &ranges, true));
    if result.is_err() {
        for process in rt.processes.iter().filter(|process| process.root != 0) {
            let _ = map_ranges(&mut rt.backend, process.root, &ranges, false);
        }
    } else {
        STAGED_CODE.with(|stored| *stored = ranges);
    }
    rt.backend.flush_mappings();
    result
}

pub fn abort(rt: &mut Runtime<PhysicalBackend>) {
    let ranges = STAGED_CODE.with(core::mem::take);
    for process in rt.processes.iter().filter(|process| process.root != 0) {
        map_ranges(&mut rt.backend, process.root, &ranges, false)
            .expect("restore replacement staging permissions");
    }
    rt.backend.flush_mappings();
}

pub fn reclaim_old_kernel(rt: &mut Runtime<PhysicalBackend>, old: KernelRange) {
    for process in rt.processes.iter().filter(|process| process.root != 0) {
        map_ranges(&mut rt.backend, process.root, &[old], false)
            .expect("restore reclaimed kernel RAM permissions");
    }
    rt.backend.flush_mappings();
}
