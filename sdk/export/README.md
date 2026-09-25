# BexOS SDK 0.2.0

This archive is the Bazel/Bzlmod SDK for signed BexOS applications, services,
and D1 drivers. Load public rules from `@bexos_sdk//rules:defs.bzl`. Manifests
are protobuf text files, and every archive rule requires an explicit signing
key. Portable service components implement the public lifecycle bindings from
`@bexos_sdk//rust:bexos_wasm_guest`; native services use the startup-channel
component/startup and versioned migration bindings in
`@bexos_sdk//rust:bexos_component`. `bexos_service` and `bexos_driver` support
freestanding or std-linked Rust, FIDL crates, and C `link_deps`. Dual-architecture
sysroots are under `//sysroot`. Runnable examples are under
`//examples`. See the BexOS RFC 0069 current-state documentation for product
import and verification details.
