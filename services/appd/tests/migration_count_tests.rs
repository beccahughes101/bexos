use bexos_appd::guest::state::{
    AppdState, LAUNCH_BASE, LaunchRecord, ManagedService, SERVICE_BASE,
};
use bexos_userspace::live_migration::State;

#[test]
fn graphical_session_checkpoint_accepts_the_full_runtime_record_capacity() {
    let mut source = AppdState::empty();
    for i in 0..48 {
        source.services.push(ManagedService {
            package: format!("bexos.test.service{i}"),
            process: "main".into(),
            process_handle: i + 1,
            space_handle: 101 + i,
            thread_handle: 201 + i,
            manager: 301 + i,
            migration: 401 + i,
            hardware: 0,
            generation: 7,
            archive: 0,
            archive_len: 0,
        });
        source.launches.push(LaunchRecord {
            package: format!("bexos.test.app{i}"),
            process: "main".into(),
            process_handle: 501 + i,
            space_handle: 601 + i,
            thread_handle: 701 + i,
            manager: 801 + i,
            progress: 0,
            uid: 1000,
            job_token: 0,
        });
    }
    let mut target = AppdState::empty();
    for key in core::iter::once(0)
        .chain((0..48).map(|i| SERVICE_BASE + i))
        .chain((0..48).map(|i| LAUNCH_BASE + i))
    {
        let bytes = source.encode_record(key).unwrap().unwrap();
        target.adopt_record(key, Some(&bytes)).unwrap();
    }
    assert_eq!(target.services.len(), 48);
    assert_eq!(target.services[47].migration, 448);
    assert_eq!(target.launches.len(), 48);
    assert_eq!(target.launches[47].uid, 1000);
    assert_eq!(target.launches[47].manager, 848);

    // The header must still reject snapshots above the supported capacity.
    source.services.push(source.services[0].clone());
    let bytes = source.encode_record(0).unwrap().unwrap();
    assert!(AppdState::empty().adopt_record(0, Some(&bytes)).is_err());
    source.services.pop();
    source.launches.push(source.launches[0].clone());
    let bytes = source.encode_record(0).unwrap().unwrap();
    assert!(AppdState::empty().adopt_record(0, Some(&bytes)).is_err());
}
