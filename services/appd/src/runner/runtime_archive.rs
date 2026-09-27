//! Legacy component-owned runner path.
//!
//! RFC 72 assigns runner selection exclusively to the platform registry. Any
//! package containing this former nested override is rejected by install and
//! migration validation; no code reads or verifies it as a runner anymore.
pub const ARCHIVE_PATH: &str = ".bexos/wasm_runner.bex";

pub fn contains_component_override(bytes: &[u8]) -> bool {
    bexos_app_archive::OpenArchive::parse(bytes)
        .ok()
        .is_some_and(|archive| archive.find(ARCHIVE_PATH).is_some())
}
