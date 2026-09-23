pub mod cas;
pub mod credentials;
#[cfg(feature = "guest")]
mod migration;
pub mod oci;
pub mod payload;
pub mod resolution;
#[cfg(feature = "guest")]
pub mod runtime;
pub mod secure_state;
pub mod transfers;
pub mod transport;
#[cfg(feature = "guest")]
mod wire;
pub use pkg_fidl::PackageStatus as Error;
pub type Result<T> = core::result::Result<T, Error>;
