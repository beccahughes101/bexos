use crate::arch::ArchAPI;
use bexos_boot::{PAGE, UPDATE_BASE, UPDATE_END};
use bexos_kernel_core::{loader::LoadPlan, transplant::KernelRange};
pub fn validate(
    artifact: &[u8],
    old: KernelRange,
) -> Result<(LoadPlan, KernelRange), &'static str> {
    let plan = LoadPlan::parse(artifact, bexos_elf::arch::Machine::current_guest())
        .map_err(|_| "replacement ELF rejected")?;
    let staging = KernelRange::new(UPDATE_BASE, UPDATE_END - UPDATE_BASE);
    if staging.overlaps(&old) {
        return Err("replacement staging overlaps live kernel");
    }
    let mut end = UPDATE_BASE;
    let mut entry_executable = false;
    for (index, seg) in plan.segments.iter().enumerate() {
        if seg
            .file_offset
            .checked_add(seg.file_size)
            .is_none_or(|v| v > artifact.len() as u64)
        {
            return Err("replacement file range rejected");
        }
        let file = KernelRange::new(seg.vaddr, seg.file_size);
        if seg.file_size > 0 && !staging.contains(file) {
            return Err("replacement load range rejected");
        }
        let mut segment_end = file.end().ok_or("replacement range overflow")?;
        if let Some(zero) = seg.zero_fill {
            let bss = KernelRange::new(zero.vaddr, zero.size_bytes);
            if bss.len > 0 && !staging.contains(bss) {
                return Err("replacement bss range rejected");
            }
            segment_end = segment_end.max(bss.end().ok_or("replacement range overflow")?);
        }
        let range = KernelRange::new(seg.vaddr, segment_end - seg.vaddr);
        for prior in &plan.segments[..index] {
            let prior_end = prior
                .zero_fill
                .map_or(prior.vaddr + prior.file_size, |bss| {
                    bss.vaddr + bss.size_bytes
                });
            if range.overlaps(&KernelRange::new(prior.vaddr, prior_end - prior.vaddr)) {
                return Err("overlapping replacement segments");
            }
        }
        end = end.max(segment_end);
        entry_executable |=
            seg.rights & 8 != 0 && file.contains(KernelRange::new(plan.entry_vaddr, 4));
    }
    if !entry_executable || end == UPDATE_BASE {
        return Err("replacement entry not executable");
    }
    let header = plan
        .segments
        .iter()
        .find(|s| s.vaddr == UPDATE_BASE && s.file_size >= 40)
        .ok_or("replacement preparation ABI missing")?;
    let bytes = artifact
        .get(header.file_offset as usize..header.file_offset as usize + 40)
        .ok_or("truncated preparation ABI")?;
    let word = |i: usize| u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap());
    if word(0) != super::prepare::API_MAGIC
        || word(1) != super::prepare::API_VERSION
        || word(4) != bexos_boot::ARCHITECTURE_ID
    {
        return Err("incompatible replacement preparation ABI");
    }
    if !plan.segments.iter().any(|s| {
        s.rights & 8 != 0
            && word(2) >= s.vaddr
            && word(2)
                .checked_add(4)
                .is_some_and(|pc| pc <= s.vaddr + s.file_size)
    }) || word(3) % 16 != 0
        || word(3) <= UPDATE_BASE + 4096
        || word(3) > end
    {
        return Err("invalid replacement preparation entry or stack");
    }
    let end = end
        .checked_add(PAGE - 1)
        .ok_or("replacement range overflow")?
        & !(PAGE - 1);
    if end + crate::memory::HEAP_BYTES as u64 > UPDATE_END {
        return Err("replacement heap exceeds staging reservation");
    }
    Ok((plan, KernelRange::new(UPDATE_BASE, end - UPDATE_BASE)))
}
pub fn load(plan: &LoadPlan, artifact: &[u8]) {
    for seg in &plan.segments {
        unsafe {
            core::ptr::copy_nonoverlapping(
                artifact.as_ptr().add(seg.file_offset as usize),
                seg.vaddr as *mut u8,
                seg.file_size as usize,
            );
            let file_end = seg.vaddr + seg.file_size;
            let padding = (PAGE - file_end % PAGE) % PAGE;
            core::ptr::write_bytes(file_end as *mut u8, 0, padding as usize);
            if let Some(bss) = seg.zero_fill {
                core::ptr::write_bytes(bss.vaddr as *mut u8, 0, bss.size_bytes as usize);
            }
        }
    }
    for seg in &plan.segments {
        crate::arch::CurrentArch::synchronize_code(seg.vaddr, seg.file_size);
    }
}
