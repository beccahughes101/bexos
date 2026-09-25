# Testing And Troubleshooting

Validation has three layers: build the external workspace using only the
unpacked SDK, verify product import/policy, and exercise the package under the
target product. A successful archive build proves packaging, not boot,
capability routing, device binding, or migration continuity.

## Package-author checks

Build every released target through Bazel:

```sh
bazel build //:wasm_app
bazel build //:native_aarch64_app //:native_x86_64_app
bazel build //:service_aarch64 //:service_x86_64
bazel build //:driver_aarch64 //:driver_x86_64
```

Locate outputs without assuming a `bazel-bin` path:

```sh
bazel cquery --output=files //:service_aarch64
```

Verify the signed archive:

```sh
bazel run @bexos_sdk//tools:bex_archive -- inspect \
  --archive /absolute/path/to/service.bex \
  --public-key /absolute/path/to/signing-root.prototxt
```

Format Rust through the pinned ruleset:

```sh
bazel run @rules_rust//:rustfmt
```

CI should provide the private signing key as a secret file, build the initial
and replacement archives, retain release digests, and publish only the signed
archives plus public signing metadata.

## BexOS SDK smoke matrix

In a BexOS source checkout, these commands build `//sdk:bexos_sdk`, unpack it
into a temporary directory, copy the external fixture, replace its SDK/module
BUILD files, and build its full archive set using only the unpacked module:

```sh
bazel run //testing/out_of_tree_sdk:acceptance -- aarch64 smoke
bazel run //testing/out_of_tree_sdk:acceptance -- x86_64 smoke
```

Expected final messages are:

```text
out-of-tree SDK fixture built for aarch64
out-of-tree SDK fixture built for x86_64
```

This proves the exported rules, toolchains, FIDL generation, WASM/native apps,
`std`/libc plus C linking, D1 driver packaging, and replacement packaging. It
does not boot QEMU.

## Product-level QEMU acceptance

Run the full acceptance wrapper on a host with the QEMU prerequisites:

```sh
bazel run //testing/out_of_tree_sdk:acceptance -- aarch64 qemu
bazel run //testing/out_of_tree_sdk:acceptance -- x86_64 qemu
```

The wrapper builds the external archives, overlays them into the SDK acceptance
product, starts the matching guest, and runs:

- `//testing/e2e/qemu/sdk:sdk_acceptance_aarch64`
- `//testing/e2e/qemu/sdk:sdk_acceptance_x86_64`

The acceptance product attaches an e1000e PCI device and requires these cold
startup markers:

```text
sdk-fixture-service: BootFS std/libc+C service started value=42
sdk-fixture-driver: e1000e bound with structured resources
```

It asserts that the early service marker appears before `appd: pivot complete`,
then stages service generation 201 and driver generation 202. Successful
replacement requires:

```text
sdk-fixture-service: replacement active with continuity
sdk-fixture-driver: replacement active with resource continuity
sdk-fixture: version-1 state and handles adopted
```

For x86 firmware builds on a host with limited system storage, set
`BEXOS_BAZEL_OUTPUT_USER_ROOT` to an absolute path on a larger volume before
running the wrapper.

## Common build and import failures

### `IncompatibleAbi`

`min_bexos_abi_version` is missing, zero, or greater than API level 1. Set it
to `1` for SDK 0.2 packages.

### `NativeArchitectureRequired` or `IncompatibleArchitecture`

A native manifest was not stamped for a concrete architecture, the wrong
archive was selected, or an ELF machine does not match the manifest/product.
Build distinct AArch64 and x86-64 targets and select them with product
configuration. Do not use `MULTI` for ELF content.

### `MissingHeartTransplant`

A process has `service: true` without
`lifecycle { update_strategy: HEART_TRANSPLANT }`. Add the declaration and the
actual runtime implementation; changing only the manifest will build a package
that cannot hand over safely.

### `InvalidComponentType`

The import role disagrees with the manifest: for example, an application has
driver metadata or a service package includes a command process. Match the
package shape to `application`, `service`, or `driver` rather than weakening
the import role.

### `InvalidDriver`

Check exact package/driver identity, one service process, at least one bind
rule, bounded `max_instances_per_host`, nonempty bind properties, and required
resource count/kinds. Also check that the product supplies both native-runner
and driver grants.

### `InvalidBootWave`

`BOOTFS` import omitted `boot_wave`, its value differs from the signed service
wave, multiple service processes disagree, or a system-image import supplied a
wave. The product cannot rewrite this signed ordering decision.

### Signing-root or signer-ID mismatch

The archive key ID/public key does not match the supplied root, or the root's
`anchor_id` differs from `signer_id`. Confirm the author handed off the public
record corresponding to the private release key and that the product trusts
the same root identity.

### Missing exact runner or driver grant

Add `<package-id>@<signer-id>` to `native_runner_grants`; add the same exact
identity to `driver_grants` for a driver. Prefix or signer-only grants do not
satisfy this check.

### Unsafe archive path or invalid ELF payload

Data entries must be normalized relative paths. Remove absolute paths, empty
components, and `..`. Rebuild native payloads with the SDK target platform; do
not insert host ELF files into `data` or manual archive entries.

## Runtime failures

- **No readiness:** confirm the process decodes `Startup`, initializes only
  from granted resources, and calls `Startup::ready` on cold start.
- **No service route:** compare provider `services_exposed` with consumer
  `services_consumed`, including visibility, capability names, required method
  ordinals, permissions, and provider process.
- **Driver does not bind:** inspect device bus/property values, bind priority,
  required resource availability, signing root, and exact driver grant.
- **Migration never completes:** poll `Source` regularly, bound normal work,
  mark dirty keys, stop mutations during quiescence, and make
  `quiescence_ready` reflect real device drain state.
- **Migration aborts during adoption:** check record versions, record sizes,
  resource descriptors, `validate`/`finish_adoption`, and service-contract
  compatibility. The old owner should remain live on a precommit failure.
- **Replacement works until reboot:** confirm the update reached durable commit
  and the accepted generation/archive record, rather than treating a successful
  stage request as completion.

## Release gate

A release is ready only when:

1. All architecture targets and replacement targets build from the unpacked
   released SDK.
2. Every `.bex` verifies against the handed-off public root.
3. Product analysis accepts role, identity, architecture, placement, and exact
   grants.
4. The package starts and exposes its intended behavior under the real product.
5. Service/driver replacement preserves state and resources, abort behavior is
   safe, and accepted selection survives reboot where persistence is required.

See the repository's [testing status](../testing-status.md) for platform-wide
coverage; the existence of a target is not by itself evidence that a product
scenario has passed.
