# RFC 0058: TUF metadata in OCI registries — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0058](README.md)

## Implementation summary

TUF metadata verification and generic HTTPS helpers exist, but the OCI-backed metadata/payload transport described here is not implemented.

## Implemented behavior

- Lib/tuf accepts supplied metadata sets and verifies role signatures, versions, references, and target content.
- Updated can use an in-memory seeded feed. Lib/distribution separates URL/policy handling from an optional live HTTPS adapter; neither supplies an OCI artifact protocol.

## Gaps and deviations

- No OCI registry client for role manifests/tags, descriptor/media-type resolution, blob fetching, authentication challenges, or role publication pipeline was found in the reviewed tools/services/libraries.
- The specified tuf-root/tuf-timestamp/tuf-snapshot mapping and digest-pinned fetch sequence remain design examples.
- TUF’s client library alone does not establish OCI tag-race/freshness protection. Updated’s time-zero feed check and lack of live delivery also apply; see RFC 0025.

## Sources and validation

Implementation and contract evidence: [lib/tuf/src/lib.rs](../../../lib/tuf/src/lib.rs), [lib/distribution/src/lib.rs](../../../lib/distribution/src/lib.rs), [lib/distribution/BUILD.bazel](../../../lib/distribution/BUILD.bazel), [services/updated/src/service.rs](../../../services/updated/src/service.rs).

No dedicated implementation test for this RFC was found in the reviewed tree.

Detailed guides and previously recorded validation: [secure runtime updates](../../secure-runtime-updates.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
