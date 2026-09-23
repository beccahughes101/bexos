# RFC 0058: TUF metadata in OCI registries — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0058](README.md)

## Implementation summary

RFC 0064 adds a pkgd OCI transport and strengthens the shared TUF verifier. Its current implementation and outstanding guest acceptance are tracked in [RFC 0064 CURRENT](../0064/CURRENT.md). The older runtime updater remains a separate consumer.

## Implemented behavior

- Lib/tuf accepts supplied metadata sets and verifies role signatures, versions, references, and target content.
- Updated can use an in-memory seeded feed. Lib/distribution separates URL/policy handling from an optional live HTTPS adapter; neither supplies an OCI artifact protocol.

## Gaps and deviations

- `services/pkgd/src/oci.rs` implements role-manifest discovery, descriptor/media-type validation, digest-pinned blobs and bearer challenges. No role publication pipeline or completed guest OCI acceptance is claimed.
- Pkgd uses sequential `tuf-root-N`, mutable `tuf-timestamp`, and authenticated SHA-256 references for snapshot/targets/payloads. Product configuration provides the initial trusted root.
- Host HTTPS fixtures now exercise signed OCI/TUF, bearer and mTLS flows, but no guest has demonstrated verified artifact delivery. An ARM application download reached 294,908 of 361,234 bytes before timing out. Configurable deadlines have host coverage; their guest result is unconfirmed. See [RFC 0064's current gaps](../0064/CURRENT.md#current-gaps) for transport, resource ownership and lifecycle limitations.
- TUF’s client library alone does not establish OCI tag-race/freshness protection. Updated’s time-zero feed check and lack of live delivery also apply; see RFC 0025.

## Sources and validation

Implementation and contract evidence: [lib/tuf/src/lib.rs](../../../lib/tuf/src/lib.rs), [lib/distribution/src/lib.rs](../../../lib/distribution/src/lib.rs), [lib/distribution/BUILD.bazel](../../../lib/distribution/BUILD.bazel), [services/updated/src/service.rs](../../../services/updated/src/service.rs).

`//services/pkgd:tests` now exercises signed OCI metadata through an in-process transport fixture; it does not establish HTTPS/HTTP2 or Trusty guest acceptance.

Detailed guides and previously recorded validation: [secure runtime updates](../../secure-runtime-updates.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
