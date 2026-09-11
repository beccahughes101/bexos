# BexOS FIDL vs Fuchsia FIDL

This document compares BexOS FIDL v1 with upstream Fuchsia FIDL.

BexOS deliberately borrows FIDL's broad shape: libraries, protocols, typed payloads, generated bindings, and an efficient binary wire contract. It is not source-compatible or wire-compatible with Fuchsia FIDL. The BexOS dialect is smaller, Rust-first, permission-aware, and tuned for BexOS appd capability brokering.

## Goals

| Area | BexOS FIDL v1 | Fuchsia FIDL |
| --- | --- | --- |
| Primary role | Typed BexOS service and capability contracts. | Typed IPC protocols for the Fuchsia platform. |
| Compatibility goal | Stable BexOS ABI and deterministic Rust generation. | Full Fuchsia language, ABI, bindings, and platform compatibility. |
| Policy model | Method-level `@permission` strings are first-class compiler metadata. | Access control is handled by Fuchsia component capability routing, not by a built-in method permission split in the language. |
| Runtime target | BexOS kernel/appd channel runtime, beginning with the appd broker. | Zircon channels and Fuchsia component framework. |
| Language backends | Backend-neutral compiler structure, Rust only today. | Production bindings across supported Fuchsia languages, including C++, Rust, and Go. |

## Source Syntax

Fuchsia FIDL uses the modern declaration form:

```fidl
type Item = struct {
    key string:128;
};
```

BexOS v1 currently uses direct declaration keywords:

```fidl
struct Item {
    key string:128;
};
```

Important syntax differences:

- BexOS supports `alias Name = Type;`; Fuchsia supports aliases, but most type declarations use `type Name = layout`.
- BexOS supports `struct`, `resource struct`, `table`, `resource table`, `enum`, `bits`, `protocol`, and `alias` as direct declarations.
- BexOS does not implement Fuchsia `const`, `union`, `service`, `compose`, `error` method result syntax, or `strict`/`flexible` modifiers.
- BexOS supports `using library.name;` but not `using library.name as alias;`.
- BexOS supports `//` and `/* ... */` comments. It does not yet propagate `///` doc comments into generated bindings.
- BexOS identifiers currently accept ASCII letters, digits, and underscores. Fuchsia has stricter naming and canonical-name collision rules.

## Protocols And Methods

Both dialects describe protocols containing methods with typed request and response payloads.

BexOS additions:

- Methods may carry `@permission("...")`.
- Capability groups are derived from exact permission strings.
- Generated metadata exposes protocol names, method names, ordinals, permissions, and capability groups.
- Explicit ordinals can be written as `1: Method(...)`; absent ordinals are deterministically assigned from protocol and method names.

Fuchsia differences:

- Protocol openness and evolution are modeled with concepts such as `open`, `ajar`, `closed`, `strict`, and `flexible`.
- Method error syntax is part of the language.
- Method selectors, generated APIs, and compatibility behavior follow Fuchsia's ABI/API evolution rules.
- Access to protocols is normally routed at the component capability level, not split into generated per-permission protocol traits.

## Type System

Shared concepts:

- Primitive scalar types: `bool`, signed and unsigned integers, and floats.
- Strings and vectors with optional bounds.
- Arrays.
- Structs, tables, enums, bits, aliases.
- Resource-oriented handles/endpoints.
- Nullable reference-like types.

BexOS v1 intentionally omits or limits:

- `union`.
- Constants and constant expressions beyond enum/bits numeric values.
- Fuchsia `byte` spelling; use `uint8`.
- Fuchsia `zx.handle:SUBTYPE` spelling; use `handle:SUBTYPE`.
- Full cross-library symbol resolution. Imported libraries are recorded, but validation currently resolves local named types.
- Fuchsia strict/flexible evolution modes for enums, bits, methods, tables, and unions.

BexOS validation rejects nullable primitive scalars, unknown local named types, duplicate declarations, duplicate method names, duplicate explicit ordinals, invalid table ordinals, invalid enum/bits backing types, and generated Rust name collisions.

## Permissions And Capability Splitting

This is the largest semantic difference.

In BexOS FIDL, `@permission` is a method-level contract:

```fidl
protocol CameraController {
    GetStatus();

    @permission("CAMERA")
    CaptureFrame() -> (struct {
        frame vector<uint8>;
    });
};
```

The compiler emits capability metadata:

- `Public` for methods without permissions.
- One deterministic capability group per exact permission string.
- The original permission expression as an opaque string.
- Method names and ordinals inside each group.

The appd evaluates the initial bind-time subset before handing out
channels. The compiler does not evaluate CEL and does not enforce policy at
runtime.

Fuchsia FIDL does not have this BexOS method-permission grouping model. Fuchsia's platform uses component manifests and capability routing to decide which components may access protocols.

## Wire ABI

BexOS v1 defines its own ABI:

- Little-endian scalar fields.
- Dispatch by 64-bit method ordinal.
- Fixed fields encoded inline in declaration order.
- Payload fixed headers padded to 8-byte alignment.
- `string` and `vector<uint8>` represented by inline offset/length descriptors.
- Handles and endpoints passed out-of-band; inline fields store a `uint32` handle-table index.
- Generated encode/decode functions operate over caller-provided byte buffers and handle tables.

Fuchsia FIDL's wire format is different and more complete:

- Messages consist of an inline primary object followed by out-of-line secondary objects in traversal order.
- Encoding is canonical and designed for in-place encode/decode.
- Pointers and handles are transformed into presence markers for transfer.
- Transactional messages include transport-level metadata used by Fuchsia bindings and transports.
- Fuchsia bindings must perform a larger set of integrity checks, including object accounting, handle accounting, recursion depth, enum/union validity, and presence marker validity.

BexOS does not currently claim Fuchsia wire compatibility. Matching Fuchsia's wire ABI would be a separate compatibility project, not a v1 compiler feature.

## Rust Bindings

BexOS Rust generation:

- Emits `#![no_std]`.
- Generates borrowed payload structs for strings and byte vectors.
- Avoids heap allocation in generated encode/decode APIs.
- Uses caller-provided buffers and handle slices.
- Emits abstract `FidlTransport` hooks rather than depending on a concrete channel runtime.
- Generates server traits and clients per permission-derived capability group.

Fuchsia bindings:

- Are part of a multi-language ecosystem.
- Provide language-specific runtime support libraries.
- Include higher-level async/sync invocation patterns depending on the language.
- Support standalone encode/decode APIs as a formalized layer separate from transport.

## Build Integration

BexOS:

- Uses Bazel only.
- `bexos_fidl_rust` invokes `//tools/fidlc:fidlc` in the execution configuration.
- Generated Rust stays in Bazel output directories and is not committed.
- The compiler's module layout is ready for future language backends, but only Rust is exposed.

Fuchsia:

- Uses the Fuchsia build/tooling stack.
- Produces FIDL JSON IR and language-specific bindings as part of the Fuchsia SDK/platform workflow.
- Includes documentation generation and compatibility tooling outside BexOS's current scope.

## Practical Migration Notes

Porting Fuchsia FIDL to BexOS usually requires:

- Rewriting `type Name = struct { ... };` to direct BexOS declarations.
- Removing `open`, `ajar`, `closed`, `strict`, and `flexible` modifiers.
- Replacing `byte` with `uint8`.
- Replacing Fuchsia handle spelling with BexOS handle spelling.
- Avoiding unions, services, constants, protocol composition, and method `error` syntax.
- Adding BexOS `@permission` annotations where appd capability gating should happen.
- Treating generated Rust APIs as `no_std` buffer/handle based APIs, not Fuchsia runtime bindings.

Porting BexOS FIDL to Fuchsia usually requires:

- Removing or externalizing `@permission` semantics.
- Translating direct declarations into Fuchsia's `type` declaration form.
- Replacing BexOS deterministic implicit ordinals with Fuchsia-compatible method evolution rules.
- Reworking generated-code expectations around Fuchsia bindings and transports.

## Summary

BexOS FIDL v1 is a focused dialect for BexOS service contracts. It keeps the familiar FIDL shape but deliberately chooses a smaller grammar, first-class permission metadata, deterministic Rust output, `no_std` generated APIs, and a BexOS-owned wire ABI. Fuchsia FIDL is a broader, mature platform IPC language with a larger grammar, stronger cross-language ecosystem, Fuchsia-specific runtime integration, and its own wire-compatibility rules.
