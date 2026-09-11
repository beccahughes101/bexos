# RFC 0041: Repository layout and Bazel product assembly — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0041](README.md)

## Implementation summary

The repository implements shared product bundles, architecture-selected Bazel builds, and prototxt/Starlark assembly for QEMU products.

## Implemented behavior

- Portable base/graphics bundles are separate from QEMU hardware configuration. Per-architecture base manifests feed nongui, workstation, and input-test products.
- Build platforms and rules select kernel/native/WASM artifacts. Product Starlark composes the same protobuf assembly schema, while app/board/policy sources remain prototxt.
- D1 drivers are organized by hardware class; D2 has a smoke target. Service/driver build definitions include migration replacement artifacts where implemented.

## Gaps and deviations

- The design tree includes unimplemented physical boards, smartphone products, runtime classes, and drivers; directory layout examples are not claims those targets exist.
- Bazel source declarations alone do not prove every architecture/product builds or meets runtime acceptance. x86 secure integration has separate unresolved gates.
- Generated protobufs, bindings, firmware, and image outputs remain build artifacts and are not part of this documentation change.

## Sources and validation

Implementation and contract evidence: [build/platforms/BUILD.bazel](../../../build/platforms/BUILD.bazel), [build/platforms/architecture.bzl](../../../build/platforms/architecture.bzl), [build/rules/assembly.bzl](../../../build/rules/assembly.bzl), [tools/assembly](../../../tools/assembly), [device/base](../../../device/base), [device/virtual/qemu](../../../device/virtual/qemu).

Relevant test sources and Bazel targets: [tools/assembly/BUILD.bazel](../../../tools/assembly/BUILD.bazel), [testing/build/architecture/BUILD.bazel](../../../testing/build/architecture/BUILD.bazel), [device/virtual/qemu/base/validate_product.sh](../../../device/virtual/qemu/base/validate_product.sh).

Detailed guides and previously recorded validation: [build assembly tooling](../../build-assembly-tooling.md), [qemu product](../../qemu-product.md), [x86_64 support](../../x86_64-support.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
