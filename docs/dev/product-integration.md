# Product Integration

This page is for the BexOS product integrator receiving a signed out-of-tree
archive. Import happens in a BexOS product workspace because the verifier and
assembly rules are product-owned, not part of the package-authoring SDK.

An import is passive: Bazel verifies signed bytes and manifest policy, then
places either the unchanged archive in `STORAGE` or the verified package tree
in BootFS. It never executes package code, runs hooks, rewrites defaults, or
resigns the package.

## Pin the archive as content

Use a repository rule with a release digest, or an equivalently immutable
internal artifact target:

```starlark
http_file = use_repo_rule(
    "@bazel_tools//tools/build_defs/repo:http.bzl",
    "http_file",
)

http_file(
    name = "acme_echo_aarch64",
    urls = ["https://downloads.example/echo-1.2.3-aarch64.bex"],
    sha256 = "<verified release sha256>",
    downloaded_file_path = "echo.bex",
)
```

Do not use a repository rule that executes vendor package code to discover or
transform the archive.

## Import a current component

```starlark
load(
    "//build/rules:prebuilt_app.bzl",
    "bexos_prebuilt_component",
)

bexos_prebuilt_component(
    name = "acme_echo",
    archive = "@acme_echo_aarch64//file",
    package_id = "com.acme.echo",
    signing_root = "//product/roots:acme.prototxt",
    signer_id = "acme-production-root",
    component_type = "service",
    placement = "SYSTEM_IMAGE",
    autoinstall = True,
)
```

`component_type` is `application`, `service`, or `driver`. The importer checks
that the signed manifest has the corresponding role. It also verifies the
expected package ID, API level, guest architecture, every ELF payload machine,
heart-transplant policy, driver identity/resources/bind rules, archive paths,
signature, and expected signing-root identity.

The older `bexos_prebuilt_app` wrapper remains available for system-image
applications:

```starlark
load("//build/rules:prebuilt_app.bzl", "bexos_prebuilt_app")

bexos_prebuilt_app(
    name = "acme_diagnostics",
    archive = "@acme_diagnostics//file",
    package_id = "com.acme.diagnostics",
    public_key = "//product/roots:acme.prototxt",
    autoinstall = True,
)
```

Prefer `bexos_prebuilt_component` when role provenance or BootFS placement is
required. Keep the compatibility wrapper for an existing application-only
product flow.

## Choose placement

### `SYSTEM_IMAGE`

- Default placement for ordinary external software.
- Stores the exact signed archive at `pkg/<package-id>.bex` in encrypted
  `STORAGE`.
- Adds the package to the system-image base set.
- `autoinstall = True` also adds it to autoinstall metadata.
- Must not specify `boot_wave`.

### `BOOTFS`

- Reserved for explicitly product-authorized early services and drivers.
- Extracts the verified package tree below `/boot/pkg/<package-id>/`.
- Requires `boot_wave`, which must exactly match every signed service process's
  wave.
- Requires `autoinstall = False`.
- Is not a shortcut around signing, native-runner, driver, or lifecycle policy.

Example early service:

```starlark
bexos_prebuilt_component(
    name = "acme_early_echo",
    archive = "@acme_echo_aarch64//file",
    package_id = "com.acme.echo",
    signing_root = "//product/roots:acme.prototxt",
    signer_id = "acme-production-root",
    component_type = "service",
    placement = "BOOTFS",
    boot_wave = 2,
    autoinstall = False,
)
```

## Authorize exact execution

External native services/drivers require an exact runner grant formed as
`<package-id>@<signer-id>`. Drivers require a second exact driver grant. For a
product macro exposing the QEMU integration shape:

```starlark
qemu_product(
    name = "acme_product",
    # Product-owned bundles and manifests omitted.
    prebuilt_apps = [":acme_echo", ":acme_nic"],
    native_runner_grants = [
        "com.acme.echo@acme-production-root",
        "com.acme.driver.nic@acme-production-root",
    ],
    driver_grants = [
        "com.acme.driver.nic@acme-production-root",
    ],
)
```

The public root must also be present and authorized in the target product's app
signing roots. Native applications must satisfy the product's native-runner
policy as well. A signature authenticates bytes; it does not itself authorize
native execution, driver binding, a package prefix, or early placement.

Product assembly rejects duplicate external package IDs, duplicate storage
destinations, collisions with in-tree packages, incompatible library
dependencies, missing exact grants, and placement contradictions.

## Architecture selection

Select the signed archive matching the product guest:

```starlark
load("//build/platforms:architecture.bzl", "guest_select")

bexos_prebuilt_component(
    name = "acme_echo",
    archive = guest_select(
        "@acme_echo_aarch64//file",
        "@acme_echo_x86_64//file",
    ),
    package_id = "com.acme.echo",
    signing_root = "//product/roots:acme.prototxt",
    signer_id = "acme-production-root",
    component_type = "service",
)
```

Do not relabel one architecture's bytes as the other. The importer compares
both manifest architecture and ELF machine. Portable WASM archives remain
`MULTI` and can be shared.

## Integrate replacements

Replacement archives are verified packages with the same stable identity and
a compatible migration reader. Stage them as immutable product/test content;
do not overwrite the installed baseline archive. Appd stages a verified
replacement under a content-addressed package-store ID, transfers state, and
only then durably selects the accepted generation.

The SDK QEMU fixture supplies replacements through `prebuilt_updates` and
invokes `update.apply_stored_service`. A production product may use a different
verified distribution path, but it must preserve signature, package identity,
generation monotonicity, and durable commit semantics.

## Integration checklist

- Pin the archive by digest and verify the expected public signing root.
- Match package ID, signer ID, role, architecture, placement, and signed wave.
- Add exact native-runner and driver grants where required.
- Keep ordinary packages in `SYSTEM_IMAGE`; justify every `BOOTFS` exception.
- Reject product-side manifest/default rewriting or resigning.
- Validate initial boot, service/driver readiness, live replacement, continuity,
  and reboot selection before release.
