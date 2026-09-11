# BexOS FIDL v1

BexOS FIDL is the source format for typed service interfaces and permission-gated capabilities. The compiler lives at `tools/fidlc`, runs through Bazel, and currently emits Rust bindings only.

The v1 compiler is intentionally BexOS-specific. It is not a full upstream Fuchsia FIDL implementation.

For a detailed comparison with upstream Fuchsia FIDL, see [BexOS FIDL vs Fuchsia FIDL](fidl-vs-fuchsia.md).

## Compiler Pipeline

`fidlc` uses a language-neutral pipeline:

1. Lex `.fidl` source into tokens.
2. Parse tokens into a typed AST.
3. Validate declarations, types, ordinals, permissions, and generated Rust names.
4. Lower into IR with deterministic method ordinals.
5. Split protocol methods into capability groups.
6. Emit backend code. Only `--rust-out` is implemented today.

Bazel invokes the compiler through `bexos_fidl_rust` in `build/rules/fidl.bzl`; generated files are build outputs and should not be committed. A target may pass either one `src` or multiple same-library `srcs` when a service contract is split across files.

## Supported Syntax

```fidl
library bexos.hardware.camera;

using bexos.media;

alias FrameBytes = vector<uint8>:4096;

enum CameraState : uint32 {
    IDLE = 0;
    STREAMING = 1;
};

bits CameraFlags : uint32 {
    HDR = 1;
    DEPTH = 2;
};

struct CameraStatus {
    ready bool;
    state CameraState;
};

table CaptureOptions {
    1: width uint32;
    2: height uint32;
};

protocol CameraController {
    1: GetStatus() -> (struct {
        ready bool;
        state CameraState;
    });

    @permission("CAMERA")
    CaptureFrame(struct {
        options CaptureOptions?;
    }) -> (struct {
        frame FrameBytes;
    });
};
```

Supported declarations:

- `library` and `using`.
- `alias`.
- `type Name = strict bits : uint32` and
  `type Name = strict enum : int32` design-dialect declarations, lowered to the
  same IR as BexOS `bits` and `enum`.
- `struct` and `resource struct`.
- `table` and `resource table`.
- `enum` and `bits` with integer backing types.
- `protocol` methods with optional explicit ordinals.
- Method attributes, including `@permission("...")` and `@ordinal(1)`.
- `//` and `/* ... */` comments.

Supported types:

- `bool`, `int8`, `int16`, `int32`, `int64`.
- `uint8`, `uint16`, `uint32`, `uint64`.
- `float32`, `float64`.
- `string` and bounded `string:MAX`.
- `vector<T>` and bounded `vector<T>:MAX`.
- `array<T, N>`.
- `handle`, `handle:SUBTYPE`, `client_end:Protocol`, and `server_end:Protocol`.
- Named types and nullable named/string/vector/handle endpoint types with `?`.

## Permissions And Capabilities

`@permission` is method-level metadata. The compiler validates that the attribute contains a non-empty string literal, preserves the expression exactly, and does not evaluate CEL or app policy.

Protocol methods are split into capability groups:

- Methods without `@permission` go into `Public`.
- Methods with the same exact permission string share one capability group.
- Capability names are deterministic PascalCase names derived from the permission string.
- Generated metadata includes method names and ordinals for bind-time policy checks.

The appd evaluates the initial bind-time subset before handing out a
channel. After bind, steady-state messages can remain peer-to-peer.

## Rust Generation

Rust output is `no_std` compatible and self-contained. It emits:

- `FIDL_LIBRARY`.
- `BEXOS_FIDL_WIRE_ABI_VERSION`.
- `PROTOCOLS`.
- `MethodBinding` and `CapabilityBinding`.
- `CAPABILITY_BINDINGS`.
- Rust types for aliases, structs, tables, enums, bits, and method payloads.
- Server traits grouped by capability.
- Client structs that call an abstract `FidlTransport`.
- `FidlEncode` and `FidlDecode` implementations for scalar fields, strings, `vector<uint8>`, handles, aliases over supported types, enums, and bits.

The generated client methods require caller-owned request/response byte buffers and handle tables. This keeps the generated API allocation-free and lets the later runtime decide where buffers live.

## Wire ABI

The v1 ABI is stable for generated BexOS bindings:

- Little-endian scalar encoding.
- Method dispatch by 64-bit ordinal.
- Explicit ordinals are preserved; missing ordinals are deterministically assigned from protocol and method names.
- Fixed-size fields are encoded inline in declaration order.
- Inline payload headers are padded to 8-byte alignment.
- `string` and `vector<uint8>` fields use a 16-byte inline descriptor: byte offset and byte length.
- Variable data starts after the aligned fixed header.
- Handles and endpoints are passed out-of-band; inline fields store a `uint32` handle-table index.

The compiler emits encode/decode hooks for generated payload types, but it does
not implement the kernel channel or scheduler. The first host-testable
appd broker and permission evaluator live in `services/appd`.

## Performance Rules

- Prefer explicit ordinals for stable external protocols.
- Prefer fixed-width scalar fields for hot paths.
- Use bounded `string` and `vector<T>` declarations when protocol contracts have natural limits.
- Use `vector<uint8>` for byte payloads that should borrow from decode buffers.
- Pass bulk data by handle when it should remain zero-copy across components.
- Keep permission groups coarse enough to avoid excessive service surfaces, but fine enough that appd bind checks match user-visible capability grants.

## Validation

The compiler rejects:

- Duplicate declaration names.
- Duplicate protocol method names.
- Duplicate explicit method ordinals.
- Zero ordinals.
- Duplicate struct/table field names.
- Duplicate or invalid table field ordinals.
- Unknown named types.
- Nullable primitive scalar types.
- Empty or non-string `@permission` values.
- Invalid enum/bits backing types.
- Bits values that are not single-bit flags.
- Rust identifier collisions after generated name conversion.

## Current Limits

- Only Rust output is implemented.
- CEL expressions are preserved by the compiler. The appd evaluates only
  `request.permissions.contains('PERM')`, `&& client.is_foreground`, and exact
  permission tokens; unsupported expressions deny.
- Nested named structs/tables are represented in Rust but complex nested encode/decode paths may return `UnsupportedType` until the runtime-facing codec grows further.
- Full upstream Fuchsia FIDL syntax and wire compatibility are not goals for v1.
