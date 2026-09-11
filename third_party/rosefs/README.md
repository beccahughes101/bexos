# roseFS provenance

BexFS builds `rosefs-core` directly through Bazel's Rust dependency resolver.
The source is pinned to upstream commit
`0cb025fed13db2cd681698bb47f5a77ff6946cbd` and retains the upstream MPL-2.0
license and notices.

BexOS does not vendor generated Cargo output. The BexOS-specific GPT partition
view, FIDL service, key handoff, durable namespace snapshot, and SYS_STATE
record live in `//drivers/d1/storage/bexos/bexfs`; they do not modify the upstream
source. Any future upstream patch must be narrow, documented here, and applied
by Bazel.
