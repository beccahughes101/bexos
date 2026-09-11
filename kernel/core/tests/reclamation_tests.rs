use super::{MemoryBackend, copy_record, encode};
use bexos_kernel_core::runtime::{Runtime, VmoBacking};
use bexos_kernel_core::transplant::codec::Reader;

fn private_pages(rt: &mut Runtime<MemoryBackend>, value: u8, flags: u32) -> (usize, Vec<u64>) {
    let handle = rt.create_vmo(3 * 4096, flags).unwrap();
    let id = rt.vmo_for(handle, 2).unwrap();
    rt.map(None, handle, 0, 3 * 4096, 0xb000_0000, 6).unwrap();
    rt.copy_to_user(0xb000_0000, &[value; 3 * 4096]).unwrap();
    rt.close(handle).unwrap();
    let pages = match &rt.vmos[id].as_ref().unwrap().backing {
        VmoBacking::LazyAnonymous { pages } => pages.iter().flatten().copied().collect(),
        VmoBacking::Contiguous { base } => (0..3).map(|i| base + i * 4096).collect(),
        VmoBacking::SharedDevice { .. } => panic!("private heap must not use host device memory"),
    };
    (id, pages)
}

fn retired_fixture(
    commit: bool,
    flags: u32,
) -> (Runtime<MemoryBackend>, usize, Vec<u64>, Vec<u64>) {
    let mut rt = Runtime::new(MemoryBackend::new(0x4500_0000));
    rt.create_process("source", "appd", 2).unwrap();
    let (old, old_pages) = private_pages(&mut rt, 0xa5, flags);
    let (target, _) = rt.create_process("replacement", "appd", 2).unwrap();
    rt.current = 1;
    let (new, new_pages) = private_pages(&mut rt, 0x5a, flags);
    rt.current = 0;
    rt.processes[0].running = true;
    rt.begin_handover(0, target, 1, 0, Default::default())
        .unwrap();
    rt.processes[1].running = true;
    rt.handover_bulk(1).unwrap();
    rt.handover_catch_up(2).unwrap();
    rt.quiesce_handover(0, 3).unwrap();
    rt.backend.require_scrub = true;
    if commit {
        rt.current = 1;
        rt.ready_handover(0, 4).unwrap();
        rt.commit_handover(5).unwrap();
        (rt, old, old_pages, new_pages)
    } else {
        rt.abort_handover().unwrap();
        (rt, new, new_pages, old_pages)
    }
}

#[test]
fn commit_and_abort_keep_retired_pages_owned_until_bounded_scrubbing() {
    for (commit, flags) in [(false, 0), (true, 0), (false, 2), (true, 2)] {
        let (mut rt, retired, pages, live) = retired_fixture(commit, flags);
        assert_eq!(rt.vmos[retired].as_ref().unwrap().refs, 0);
        assert!(rt.has_pending_reclamation());
        assert_eq!(rt.next_deadline_on_cpu(0), Some(1_000_000));
        assert!(pages.iter().all(|page| rt.backend.pages.contains_key(page)));
        let live_before: Vec<_> = live.iter().map(|p| rt.backend.pages[p]).collect();
        assert_eq!(rt.reclaim_retired_pages(0), 0);
        assert_eq!(rt.reclaim_retired_pages(1), 1);
        assert_eq!(
            pages
                .iter()
                .filter(|p| rt.backend.pages.contains_key(p))
                .count(),
            2
        );

        // Resume maintenance after a kernel checkpoint taken halfway through
        // retirement. The backend represents the same still-owned RAM.
        let bytes = encode(&rt);
        let mut restored =
            Runtime::read_snapshot(rt.backend.clone(), &mut Reader::new(&bytes)).unwrap();
        assert_eq!(restored.reclaim_retired_pages(1), 1);
        assert_eq!(restored.reclaim_retired_pages(1), 1);
        assert!(restored.vmos[retired].is_none());
        assert!(
            pages
                .iter()
                .all(|p| !restored.backend.pages.contains_key(p))
        );
        for (page, expected) in live.iter().zip(live_before) {
            assert_eq!(restored.backend.pages[page], expected);
        }
        assert_eq!(restored.reclaim_retired_pages(8), 0);
        assert!(!restored.has_pending_reclamation());
        assert_eq!(restored.next_deadline_on_cpu(0), None);

        let mut legacy = bytes;
        legacy[8..16].copy_from_slice(&13u64.to_le_bytes());
        assert!(Runtime::read_snapshot(rt.backend.clone(), &mut Reader::new(&legacy)).is_err());
    }
}

#[test]
fn pending_reclamation_and_progress_survive_incremental_kernel_handover() {
    let (mut source, retired, pages, _) = retired_fixture(true, 2);
    let mut target = Runtime::new(source.backend.clone());
    let mut cursor = source.begin_live_snapshot(1024).unwrap();
    while let Some(key) = cursor.next() {
        copy_record(&mut source, &mut target, key);
    }
    assert_eq!(source.reclaim_retired_pages(1), 1);
    while let Some(key) = source.dirty_next().unwrap() {
        copy_record(&mut source, &mut target, key);
    }
    target.validate_live_snapshot().unwrap();
    assert_eq!(encode(&source), encode(&target));
    // Frame allocator state is also transferred by the kernel checkpoint.
    target.backend = source.backend.clone();
    assert_eq!(target.reclaim_retired_pages(8), 2);
    assert!(target.vmos[retired].is_none());
    assert!(pages.iter().all(|p| !target.backend.pages.contains_key(p)));
}
