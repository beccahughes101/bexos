use super::*;
fn tables<const N: usize>() -> PageTables<N> {
    let mut result = PageTables::empty();
    result
        .initialize(
            0x100000,
            RamBank {
                start: 0x40000000,
                length: 0x80000000,
            },
        )
        .unwrap();
    result
}
#[test]
fn full_guest_banks_and_small_pages_keep_permissions_and_boundaries() {
    let mut t = tables::<10>();
    for page in 0..512 {
        t.map(
            page * 0x200000,
            0x40000000 + page * 0x200000,
            PageSize::Large,
            Permissions::RAM,
        )
        .unwrap();
    }
    assert_eq!(t.allocated_tables(), 3);
    assert_eq!(
        t.translate(0x3fffffff),
        Some((0x7fffffff, Permissions::RAM, PageSize::Large))
    );
    assert_eq!(t.translate(0x40000000), None);
    t.map(0x40001000, 0x80001000, PageSize::Small, Permissions::CODE)
        .unwrap();
    t.map(0x40002000, 0x80002000, PageSize::Small, Permissions::DATA)
        .unwrap();
    assert_eq!(
        t.translate(0x40001fff),
        Some((0x80001fff, Permissions::CODE, PageSize::Small))
    );
    assert_eq!(
        t.translate(0x40002001),
        Some((0x80002001, Permissions::DATA, PageSize::Small))
    );
    assert_eq!(
        t.map(0x40000000, 0x80000000, PageSize::Large, Permissions::RAM),
        Err(MapError::Conflict)
    );
    t.unmap(0x40001000, PageSize::Small).unwrap();
    assert_eq!(t.translate(0x40001000), None);
    assert!(t.translate(0x40002000).is_some());
    t.map(0x40001000, 0x80001000, PageSize::Small, Permissions::READ)
        .unwrap();
    assert_eq!(t.translate(0x40001000).unwrap().1, Permissions::READ);
}
#[test]
fn pool_exhaustion_and_collisions_do_not_partially_install() {
    let mut t = tables::<3>();
    assert_eq!(
        t.map(0, 0x40000000, PageSize::Small, Permissions::DATA),
        Err(MapError::NoTables)
    );
    assert_eq!(t.allocated_tables(), 1);
    t.map(0, 0x40000000, PageSize::Large, Permissions::READ)
        .unwrap();
    let before = t.translate(0);
    assert_eq!(
        t.map(4096, 0x40001000, PageSize::Small, Permissions::DATA),
        Err(MapError::Conflict)
    );
    assert_eq!(
        t.map(0x40000000, 0x80000000, PageSize::Large, Permissions::READ),
        Err(MapError::NoTables)
    );
    assert_eq!(t.translate(0), before);
    assert_eq!(t.translate(0x40000000), None);
    assert_eq!(t.unmap(4096, PageSize::Small), Err(MapError::Conflict));
}
#[test]
fn monitor_other_domain_overflow_and_alias_truncation_fail_closed() {
    let mut t = tables::<8>();
    for host in [0x100000, 0x3ffff000, 0xc0000000, u64::MAX - 4095] {
        assert_eq!(
            t.map(0, host, PageSize::Small, Permissions::RAM),
            Err(MapError::OutsideDomain)
        );
    }
    for guest in [1, 1 << 48, u64::MAX - 4095] {
        assert_eq!(
            t.map(guest, 0x40000000, PageSize::Small, Permissions::DATA),
            Err(MapError::InvalidRange)
        );
    }
    assert_eq!(t.allocated_tables(), 1);
    assert_eq!(
        t.initialize(
            0x40000000,
            RamBank {
                start: 0x40000000,
                length: 4096
            }
        ),
        Err(MapError::OutsideDomain)
    );
    assert_eq!(
        t.initialize(
            0x100000,
            RamBank {
                start: u64::MAX - 4095,
                length: 8192
            }
        ),
        Err(MapError::InvalidRange)
    );
    assert_eq!(t.allocated_tables(), 1);
}
