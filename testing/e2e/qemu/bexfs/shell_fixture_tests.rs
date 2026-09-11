extern crate alloc;
use bexos_tty::provider::ProviderSession;
use bexos_userspace::{Channel, live_migration::State};
#[path = "shell_fixture/migration.rs"]
mod migration;

#[test]
fn full_buffers_checkpoint_in_bounded_records_and_recover() {
    let mut source = migration::Runtime::empty();
    source.control = Channel(1);
    source.migration = Some(Channel(2));
    source.providers = vec![3, 4];
    source.output = (0..32768).map(|i| (i % 251) as u8).collect();
    source.input = vec![42; 4096];
    source.errors = vec![43; 4096];
    source.eof = true;
    let mut target = migration::Runtime::empty();
    for key in source.keys() {
        let bytes = source.encode_record(key).unwrap().unwrap();
        assert!(bytes.len() <= bexos_userspace::live_migration::MAX_RECORD_DATA);
        target.adopt_record(key, Some(&bytes)).unwrap();
    }
    target.validate().unwrap();
    assert_eq!(target.output, source.output);
    assert_eq!(target.input, source.input);
    assert_eq!(target.errors, source.errors);
    assert_eq!(target.providers, source.providers);
    assert!(target.eof);
    // Rollback keeps the source buffers intact; adoption never consumes them.
    assert_eq!(source.output.len(), 32768);
}
