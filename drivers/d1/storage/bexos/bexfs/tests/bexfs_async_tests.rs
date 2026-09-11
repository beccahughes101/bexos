#[test]
fn bexfs_guest_entry_is_tokio_future() {
    let _future = bexos_bexfs::guest::main(0);
}

fn block_checkpoint(block_size: u64, max_transfer_blocks: u64) -> Vec<u8> {
    [
        1,
        10,
        11,
        12,
        0x8000_0000,
        1,
        block_size,
        4096,
        max_transfer_blocks,
        0,
        7,
    ]
    .into_iter()
    .flat_map(u64::to_le_bytes)
    .collect()
}

#[test]
fn block_handover_preserves_every_transfer_page() {
    use bexos_bexfs::guest_block::SharedBlock;
    use bexos_userspace::live_migration::Resource;

    for block_size in [512, 4096] {
        let checkpoint = block_checkpoint(block_size, 512 * 1024 / block_size);
        let block = SharedBlock::adopt(&checkpoint).unwrap();
        assert_eq!(block.checkpoint(), checkpoint);
        let resources = block.resources();
        assert!(matches!(resources[0], Resource::Handle(10)));
        assert!(matches!(resources[1], Resource::Handle(11)));
        assert!(matches!(
            resources[2],
            Resource::Mapping {
                handle: 12,
                offset: 0,
                va: 0x8000_0000,
                size: 524288,
                rights: 6,
            }
        ));
        assert!(
            SharedBlock::adopt(&block_checkpoint(block_size, 512 * 1024 / block_size + 1)).is_err()
        );
    }
}
