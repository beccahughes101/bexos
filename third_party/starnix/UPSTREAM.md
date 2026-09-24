# Starnix upstream provenance

This directory vendors the Fuchsia Starnix kernel, supporting libraries, and
the upstream `hello_starnix` fixture from:

- Repository: `https://fuchsia.googlesource.com/fuchsia.git`
- Revision: `cb5f36aee5510be9565392194f341d0409381db6`
- Imported paths: `src/starnix/kernel`, `src/starnix/lib`, and
  `src/starnix/hello_starnix` (stored as `src/hello_starnix` here)

The upstream BSD license is preserved in `LICENSE`. The original upstream
crate roots are retained verbatim as `src/kernel/core/fuchsia_lib.rs` and
`src/kernel/fuchsia_main.rs`; the corresponding `lib.rs` and `main.rs` are the
BexOS platform gates used by Bazel. Upstream GN files remain dependency/source
manifests. Nested upstream `BUILD.bazel` files are preserved byte-for-byte with
the suffix `.fuchsia` so their Fuchsia-only loads do not create Bazel
subpackages in this repository. `IMPORT.prototxt` records the source mapping.

The Phase 2 BexOS configuration intentionally enables only the single-process,
static-ELF bootstrap. Fuchsia component, storage, networking, Android, and OCI
backends remain source-visible but are not enabled until their RFC phases.
