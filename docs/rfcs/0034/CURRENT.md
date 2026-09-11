# RFC 0034: Application and system extensions — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0034](README.md)

## Implementation summary

Generic FIDL service routing and WASM child execution are available foundations, but the proposed extension-point framework is not implemented.

## Implemented behavior

- App manifests can expose/consume named services; appd brokers them using capability policies.
- The WASM runtime can host restricted children and compose declared component dependencies. These mechanisms can support explicit application integrations without defining a system extension registry.

## Gaps and deviations

- No ExtensionPoint contract, extension-point declaration/registration schema, extension discovery lifecycle, or shell extension host matching the RFC was found.
- Share-sheet, keyboard, file-provider, and system-shell extension examples are not shipped extension APIs.
- Checkpoint support in a child runtime does not supply an extension host’s state protocol, update compatibility, consent, or heart transplant implementation.

## Sources and validation

Implementation and contract evidence: [idl/bexos/app/manifest.proto](../../../idl/bexos/app/manifest.proto), [services/appd/src/broker.rs](../../../services/appd/src/broker.rs), [services/appd/src/service_directory.rs](../../../services/appd/src/service_directory.rs), [lib/wasm_runtime/src/child.rs](../../../lib/wasm_runtime/src/child.rs), [apps/userui/src/desktop.rs](../../../apps/userui/src/desktop.rs).

No dedicated implementation test for this RFC was found in the reviewed tree.

Detailed guides and previously recorded validation: [appd userspace](../../appd-userspace.md), [wasm runtime](../../wasm-runtime.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
