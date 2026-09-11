# RFC 0002: Native AI architecture — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0002](README.md)

## Implementation summary

The OS has several foundations needed by this design, but no integrated native AI service or end-to-end AI workflow was found.

## Implemented behavior

- Appd routes declared FIDL services through capability and method policies. Signed packages, user-scoped storage, and a WASM runner provide infrastructure an AI service could consume.
- Dioxus WASM applications can submit native scenes; the compositor manages views and input. These are application/rendering facilities, not an AI semantic-screen feed.

## Gaps and deviations

- No AI broker, model inference/NPU service, FIDL-to-tool schema compiler, transient widget synthesis pipeline, embeddings service, or vector-memory indexing implementation was found in services, apps, libraries, or IDL.
- The compositor does not supply the proposed accessibility/AI semantic graph. Policy expressions and capability routing do not implement the proposed AI-specific CEL confirmation flow.
- Secure ConfirmationUI and a migratable AI service remain future work. The RFC’s zero-latency and zero-copy AI claims are not measured implementation results.

## Sources and validation

Implementation and contract evidence: [services/appd/src/broker.rs](../../../services/appd/src/broker.rs), [services/appd/src/policy.rs](../../../services/appd/src/policy.rs), [lib/wasm_runtime/src/lib.rs](../../../lib/wasm_runtime/src/lib.rs), [lib/dioxus_dom](../../../lib/dioxus_dom), [idl/bexos/ui/graphics.fidl](../../../idl/bexos/ui/graphics.fidl).

No dedicated implementation test for this RFC was found in the reviewed tree.

Detailed guides and previously recorded validation: [dioxus](../../dioxus.md), [appd userspace](../../appd-userspace.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
