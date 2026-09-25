# SDK examples

This directory contains portable WASM and native AArch64/x86-64 service
examples. Copy an Ed25519 BEX signing key to `signing.key`, then build one or
more packages:

```sh
bazel build //examples:wasm_service
bazel build //examples:native_service_aarch64
bazel build //examples:native_service_x86_64
```

The key file uses the BEX archive tool's text format:

```text
key_id_hex=<64 lowercase hexadecimal characters>
seed_hex=<64 lowercase hexadecimal characters>
```

The SDK deliberately contains no private signing key. The matching public key
must be authorized by the target product and supplied to
`bexos_prebuilt_app`. The complete standalone acceptance fixture in the BexOS
source tree is built by `//testing/out_of_tree_sdk:acceptance`.
