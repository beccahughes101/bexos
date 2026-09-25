# BexOS SDK 0.1.0

This archive is the Bazel/Bzlmod SDK for signed BexOS application packages.
Load the public rules from `@bexos_sdk//rules:defs.bzl`. Application manifests
are protobuf text files, and every archive rule requires an explicit signing
key. Portable service components implement the public lifecycle bindings from
`@bexos_sdk//rust:bexos_wasm_guest`; native services use the startup-channel
helpers in `@bexos_sdk//rust:bexos_app`. Runnable examples are under
`//examples`. See the BexOS RFC 0069 current-state documentation for product
import and verification details.
