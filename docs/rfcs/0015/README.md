# RFC 0015: Assembly and component configuration

- Created: 2026-08-26T22:44:44-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Prototxt schemas and Bazel assembly separate packages, products, platform policy, and component settings. Typed configuration reaches services through validated VMOs, with controlled preference updates and migration.

## Design overview

BexOS uses an assembly model to keep software packaging, product selection,
platform policy, and per-component runtime configuration separate. Source
configuration is protobuf text (`.prototxt`) and Bazel compiles it into binary
artifacts before any image is assembled.

This avoids ambient runtime parsing: apps do not discover arbitrary TOML, YAML,
or JSON files from disk. Instead, app manifests declare a typed config schema,
products provide overrides, Bazel validates the result, and `appd` passes a
validated config VMO at launch. Privileged operators can install or reset a
generation-checked override through appd/debugd; the effective snapshot is used
by subsequent launches and registered live receivers.

## Source Manifests

The assembly path is split into four source manifest families:

- App package manifests in `idl/bexos/app/manifest.proto` declare processes,
  capabilities, resource groups, driver bind rules, and optional
  `config_schema` fields.
- Assembly input bundles in `idl/bexos/platform/assembly.proto` group packages
  by product role and placement (`BOOTFS` or `SYSTEM_IMAGE`).
- Product definitions select bundles for a concrete image and provide typed
  `component_config_values` overrides by package ID.
- Board manifests still own hardware facts and platform policy through
  `device.prototxt`, `bootfs_manifest.prototxt`, `system_image.prototxt`, and
  `platform.pcfg.prototxt`.

The headless QEMU product lives under `//device/virtual/qemu/nongui`;
`//device/virtual/qemu/workstation` adds graphics bundles and native QEMU UI
configuration while sharing the same architecture-specific base. Each product’s
`products.star` source returns this protobuf-shaped value before Bazel encodes it:

```protobuf
product_name: "nongui_aarch64"
board: "//device/virtual/qemu/base/aarch64"
platform_config: "//device/virtual/qemu/nongui:platform_config_bin"
bundles: "base"
bundles: "qemu_hardware"

component_config_values {
  package_id: "bexos.platform.storage_verify"
  values {
    name: "retry_limit"
    value { uint32_value: 3 }
  }
}
```

## Typed Component Config

Apps opt in by declaring a scalar schema in their package manifest:

```protobuf
config_schema {
  fields {
    name: "enable_persistence"
    type: CONFIG_BOOL
    required: true
    default_value { bool_value: true }
  }
  fields {
    name: "channel"
    type: CONFIG_STRING
    required: true
    max_size: 16
    default_value { string_value: "stable" }
  }
}
```

The initial supported value types are `CONFIG_BOOL`, `CONFIG_UINT32`,
`CONFIG_UINT64`, `CONFIG_STRING`, and `CONFIG_BYTES`. Required fields must be
provided either by a schema default or by the selected product. String and byte
fields may set `max_size`; Bazel validation rejects oversized values.

`//tools/assembly:bexos_assembly` compiles the resolved values into a compact
binary table with a stable `BEXCFG` header. Assembly emits v2 tables with schema
fingerprint and generation metadata; userspace also accepts v1 tables for
compatibility. `lib/userspace::config::ConfigTable` provides the runtime scalar
lookup API, and `component_config_rust` can generate a Rust `ComponentConfig`
binding from the compiled manifest schema.

`platform.pcfg.prototxt` also carries update policy: TUF metadata and target
base URLs, trusted initial root key material or root metadata, app/kernel/TEE
target naming policy, metadata and target byte limits, and the default
check/stage/apply behavior. App manifests and package config remain prototxt;
runtime TUF metadata remains JSON because that is the TUF wire format.

## Bazel Flow

The public rules live in `//build/rules:assembly.bzl`:

- `assembly_input_bundle` compiles an AIB `.prototxt`.
- `component_config_override` compiles a package override `.prototxt`.
- `app_config` validates an app manifest schema plus optional override fragment
  and emits `*.bexconfig`.
- `product_app_config` extracts a package override directly from a compiled
  product definition and emits that package's `*.bexconfig`.
- `product_app_config_policy` compiles explicit product locks into a separate
  protobuf policy artifact.
- `component_config_rust` generates a Rust `ComponentConfig` binding from the
  compiled manifest schema.
- `bexos_product` validates selected bundles, package IDs, package manifest
  references, duplicate packages, unknown overrides, and config value bounds.

`app_archive` accepts an optional compiled `config` label. When present, the
archive includes it at `config/component.bexconfig`. Its optional `config_policy`
label installs `config/component.bexpolicy`.

The QEMU build validates the Starlark-generated product definition plus
`device/base/base.aib.prototxt` and `qemu_hardware.aib.prototxt` through
`:virtual_aarch64_product_assembly`. The generated BootFS label list feeds
`bootfs_manifest.prototxt` validation, the assembly index is embedded into
BootFS at `/boot/manifest/product.assembly`, and `product_app_config` compiles
the storage verifier's package config directly from the generated product definition.

## Runtime Delivery

When launching a package from the package store or an in-memory archive,
`appd` opens `/pkg/config/component.bexconfig` if it exists, validates the
`BEXCFG` table, layers any accepted override for the exact package version, and
copies the effective bytes into a VMO sent in the startup message as:

- `Startup.config`: a single optional VMO handle.
- `Startup.config_len`: the exact valid byte length.

Existing startup calls remain compatible: older messages and packages without
config receive `None` and length `0`. Appd supplies authoritative schemas,
packaged baselines, exact package keys, and instance bindings to prefsd. Prefsd
persists operator assignments by exact package key and user assignments by
package/schema fingerprint. Existing appd override records are imported during
migration without losing their revision.

`debugd` and `bexctl` expose the privileged operator path:

- `bexctl config get PACKAGE [-o PATH]`
- `bexctl config set PACKAGE EXPECTED_GENERATION NAME=TYPE:VALUE...`
- `bexctl config reset PACKAGE EXPECTED_GENERATION`

The operator and user-preference paths use a receiver-side prepare/commit
protocol. Opted-in running instances acknowledge or reject a candidate before
it is committed; older instances keep their startup snapshot until relaunch.

## Writable Fields and Shared Validation

`config_schema.fields` adds `scope`, `display_name`, `description`, and an optional
`uint_range { min, max }` or `string_enum { values }`. Omitted scope is
`SCOPE_SYSTEM_ONLY`. `SCOPE_USER_EDITABLE` permits user assignments;
`SCOPE_MDM_LOCKABLE` permits them unless a product or operator lock is active.
Unsigned bounds are inclusive and must fit the declared numeric type. String
enums must contain unique values of the declared string type and respect
`max_size`. Defaults and all runtime assignments obey the same constraints.

```protobuf
config_schema {
  fields {
    name: "dark_mode"
    type: CONFIG_BOOL
    scope: SCOPE_USER_EDITABLE
    default_value { bool_value: false }
    display_name: "Dark mode"
  }
  fields {
    name: "max_cache_mb"
    type: CONFIG_UINT32
    scope: SCOPE_MDM_LOCKABLE
    default_value { uint32_value: 256 }
    uint_range { min: 64 max: 1024 }
  }
}
```

`lib/component_config` is the shared schema, fingerprint, BEXCFG, resolver,
transaction, and durable-slot library. Assembly, appd, prefsd, and generated
bindings use it. Scope and constraints extend compatibility fingerprints;
display metadata does not. Schemas without scope/constraint extensions retain
their previous fingerprints. Legacy BEXCFG v1 tables remain accepted, with
runtime type and constraint validation. Malformed tables, duplicate fields,
unknown assignments, and incompatible v2 fingerprints are rejected.

## Layers and Explicit Policy

Resolve each field in this order:

1. Manifest defaults and the packaged assembly baseline.
2. Privileged operator assignments for the exact installed package version.
3. Permitted user assignments for the package and schema fingerprint.

Assignments are partial overlays. A product or operator value alone does not
lock a field. Explicit locks suppress stored user values without deleting them;
unlocking exposes those values again. User reset removes only user assignments.
Operator reset removes operator assignments and runtime locks, retaining the
packaged baseline and product locks.

Products declare `locked_fields` alongside `component_config_values`; Bazel
compiles them into the separate `ComponentConfigPolicy` protobuf artifact at
`config/component.bexpolicy`. Only MDM-lockable fields accept either type of lock.
Runtime unlock cannot remove a product lock. Operator lock/unlock uses the same
revision checks and receiver coordination as operator assignment changes.

## Prefsd and Encrypted Persistence

`bexos.service.prefsd` starts in wave 5 after storage and usersd are available.
Its own timeout config bootstraps from its packaged configuration. It has
separate policy/service, storage, binding, runtime, wire, and migration modules.
Appd remains authoritative for installed schemas, exact package versions,
packaged policy, and running instances. Public callers cannot register schemas,
choose an identity, attach observers, or change operator policy.

Nonzero UIDs store preferences in their encrypted user home at
`prefs/<package-id>/<schema-fingerprint>/`. UID 0 uses prefsd's system data
directory. Compatible package versions share assignments; differing fingerprints
retain independent records and begin with assembly/operator defaults. No
implicit field copying occurs across incompatible schemas.

The preference payload remains BEXCFG. Alternating `0.bexpref` and `1.bexpref`
files contain a wrapped BEXCFG record with the schema-encoded user assignments
and the operator revision checkpoint already reflected in that user's effective
generation. Each snapshot is synchronized before its commit record; commit
records identify generation, length, and checksum. Recovery chooses the latest
valid committed slot, and legacy direct-table snapshots with revision sidecars
remain readable. A failed or truncated candidate leaves the previous committed
slot intact. An unreadable store is an error, never a volatile success. A failed
commit-marker synchronization is an indeterminate decision: the service holds
and retries that exact decision instead of claiming an abort.

Effective generations advance monotonically and are persisted with their
operator-revision checkpoints. Recovery replays every missed operator revision;
it does not reconstruct generations by adding independent counters that can
repeat. Compatible versions share the preference counter, while operator revision
checks remain exact-version-specific. All mutations are serialized and stale
requests receive the current generation.

Each access checks usersd's current unlock state. User lock/deletion revokes
ordinary client bindings and observer endpoints, aborts uncommitted work, and
clears cached user records. Committed delivery is never changed into an abort.
Affected launches fail explicitly if preference resolution is invalid or
unavailable; packages without config retain their existing startup format.

## Preference and Management APIs

`idl/bexos/preferences/preferences.fidl` defines the implemented wire contract:

- `UserPreferences.GetPreferences(package_id)` returns status, effective
  generation, a read-only config VMO, schema metadata, and active locks.
- `SetPreference(package_id, key, expected_generation, typed_value)` validates
  one assignment and reports its resulting generation.
- `ResetToDefaults(package_id, expected_generation)` removes the user layer.
- The privileged `Manage` method selects an explicit UID and supports get,
  atomic assignment overlays, and reset. Ordinary methods use the UID carried
  by their authenticated binding and only access the caller's own package.

Appd issues service bindings containing package/UID identity, the exact package
key, and allowed ordinals. Ordinary calls resolve the caller’s own package
version even when compatible versions are installed in parallel.
Cross-package management requires `bexos.permission.MANAGE_USER_PREFERENCES`;
ordinary apps cannot gain it merely by declaring it. The current broker reserves
it for platform management and the privileged debugger. This interface is the
basis for a future Settings application or MDM client.

CLI access preserves existing scalar assignment syntax:

```text
bexctl prefs get PACKAGE --uid UID
bexctl prefs set PACKAGE EXPECTED_GENERATION NAME=TYPE:VALUE... --uid UID
bexctl prefs reset PACKAGE EXPECTED_GENERATION --uid UID
bexctl config lock PACKAGE EXPECTED_GENERATION FIELD...
bexctl config unlock PACKAGE EXPECTED_GENERATION FIELD...
bexctl config policy PACKAGE
```

Existing `config get/set/reset` commands remain available. Debugd and the host
client expose conflicts, lock state, and committed delivery pending.

## Live Transactions and Heart Transplant

Appd supplies an optional `Startup.config_endpoint`. Merely receiving an endpoint
does not register a receiver. Existing generated `ComponentConfig` loading APIs
continue to work; generated `LiveComponentConfig` opts in through
`ConfigObserver.Register`. Registration returns a current snapshot atomically,
covering changes between startup delivery and registration.

For preference and operator mutations:

1. Resolve and validate all affected candidate snapshots. Preference writes
   include registered receivers of every compatible installed version.
2. Allocate a distinct transaction ID for every attempt, including aborted
   attempts. Send `Prepare` with that ID and read-only candidate VMOs. Receivers
   validate and stage candidates without publishing them. All receivers must
   accept before storage changes. Stale acknowledgements cannot authorize a later
   attempt with the same effective generation. A rejection, disconnection, or
   deadline expiry aborts the transaction.
   The default prepare deadline is five seconds, configured with prefsd's
   prototxt `prepare_timeout_ms` field.
3. Synchronize the candidate and its commit decision, then publish its generation.
4. Deliver idempotent `Commit` requests. Bindings replace their current immutable
   snapshot and invoke the application callback only on commit. Readers retaining
   an older `Arc` keep a valid snapshot. `Abort` discards a staged candidate.

A failure after durable commit is reported as committed with delivery pending,
never as an aborted write. Delivery is retried and committed state survives heart
transplant. Closed receivers are retired; a subsequent launch reads the durable
snapshot. Older instances without registered receivers keep their startup values
until their next launch. Pending transactions hold launch resolution and
conflicting package operations. Prefsd's asynchronous receiver state machine
never makes reciprocal calls into appd while appd waits for resolution.

Migration quiescing rejects new mutations and aborts uncommitted transactions.
Prefsd migration records retain authenticated clients, usersd/storage connections,
user watchers, schemas, policy, generations, observers, and transaction decisions.
Records are chunked to fit the migration transport. Appd retains its authoritative
registry and instance state; its preference registration cache is reconstructed.
Committed receiver delivery resumes after activation.

## Future Designs

- Structured config types beyond the current scalar schema and richer live
  application integrations, including UI state refresh helpers.
- Explicit schema migration and version-diff tooling, with reviewed migration
  rules rather than silently copying incompatible records.
- A Settings UI consuming display metadata and the management API, and enterprise
  MDM authentication, policy distribution, and management clients.
- Config compatibility and version diffs integrated into Heart Transplant update
  policy, including richer application acceptance criteria.
- Assembly-driven BootFS and system-image construction replacing the remaining
  hand-maintained entry lists.
