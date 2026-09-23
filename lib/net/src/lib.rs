#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(all(target_arch = "aarch64", any(target_os = "linux", target_os = "none")))]
core::arch::global_asm!(
    r#"
    .section .note.gnu.property, "a", %note
    .balign 8
    .long 4
    .long 16
    .long 5
    .asciz "GNU"
    .balign 8
    .long 0xc0000000
    .long 4
    .long 3
    .long 0
    .balign 8
    .previous
"#
);

#[cfg(feature = "std")]
pub mod async_connect;
#[cfg(feature = "std")]
pub mod async_http;
pub mod doh;
pub mod http1;
pub mod http2;
pub mod http3;
pub mod http_stream;
pub mod nts;
pub mod quic;
#[cfg(feature = "std")]
pub mod secure;
pub mod tls;

pub const ABI_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetError {
    InvalidArgs,
    BufferTooSmall,
    Unsupported,
    BodyTooLarge,
    BadHttp,
    Network,
    Tls,
    MissingService,
    TimedOut,
    WouldBlock,
}
