#![no_std]
#![no_main]
extern crate alloc;
use bexos_userspace::{Channel, Startup, yield_now};
bexos_userspace::entry!(run);
fn run(channel: u64) -> ! {
    let c = Channel(channel);
    let s = Startup::receive(c).unwrap();
    assert!(s.migration_target);
    #[cfg(candidate_failure)]
    panic!("injected precommit candidate failure");
    #[cfg(candidate_timeout)]
    loop {
        yield_now();
    }
    #[cfg(candidate_incompatible)]
    {
        let _ = bexos_userspace::live_migration::receive::<Incompatible>(c, s.migration_generation);
        bexos_userspace::exit();
    }
    #[cfg(candidate_reject)]
    {
        use migration_fidl::FidlEncode;
        let _ = c.recv().unwrap();
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&s.migration_generation.to_le_bytes());
        bytes[8..16].copy_from_slice(&1u64.to_le_bytes());
        let e = migration_fidl::StateReceiverInitializeResponse { status: -8 }
            .encode(&mut bytes[16..], &mut [])
            .unwrap();
        c.send(&bytes[..16 + e.bytes], &[]).unwrap();
        loop {
            yield_now();
        }
    }
}
#[cfg(candidate_incompatible)]
struct Incompatible;
#[cfg(candidate_incompatible)]
impl bexos_userspace::live_migration::State for Incompatible {
    fn empty() -> Self {
        Self
    }
    fn keys(&self) -> alloc::vec::Vec<u64> {
        alloc::vec![]
    }
    fn encode_record(&self, _: u64) -> Result<Option<alloc::vec::Vec<u8>>, bexos_migration::Error> {
        Err(bexos_migration::Error::UnsupportedVersion)
    }
    fn adopt_record(&mut self, _: u64, _: Option<&[u8]>) -> Result<(), bexos_migration::Error> {
        Err(bexos_migration::Error::UnsupportedVersion)
    }
    fn validate(&self) -> Result<(), bexos_migration::Error> {
        Err(bexos_migration::Error::UnsupportedVersion)
    }
    fn resources(&self) -> alloc::vec::Vec<bexos_userspace::live_migration::Resource> {
        alloc::vec![]
    }
    fn activated(&mut self, _: u64) {
        panic!("incompatible candidate committed")
    }
}
