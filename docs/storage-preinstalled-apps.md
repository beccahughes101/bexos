# Storage Preinstalled Apps

QEMU storage-preinstalled packages are seeded into the `STORAGE` BexFS image
under `pkg/` and imported by appd after the storage stack is mounted.
This keeps non-boot-critical services out of BootFS while still making them
available on first boot.

The current QEMU storage-preinstalled service set includes `netstackd`,
`timed`, and `jobd`. The image configuration also includes the application
`bexos.app.brush_shell` at `pkg/bexos.app.brush_shell.bex`; unlike waved services,
its terminal process launches on demand through Opener. Both architecture
system-image manifests list it for base installation and autoinstall.

`jobd` is seeded as `pkg/bexos.service.jobd.bex`, listed in
the system-image base/autoinstall package metadata, and launched by appd at wave
7 after the time, network, power, and user services it depends on are available.

To add a new storage-preinstalled package:

1. Add or reuse a service/package manifest in prototxt form.
2. Add a portable package to `device/base/base.aib.prototxt`, or a QEMU-specific
   package to `device/virtual/qemu/base/aarch64/qemu_hardware.aib.prototxt`, with
   `placement: SYSTEM_IMAGE`.
3. Add the manifest label to `virtual_aarch64_product_assembly.manifests` in
   `device/virtual/qemu/base/images.bzl`.
4. If product config is needed, add a `product_app_config` target and pass it to
   the app archive used by QEMU.
5. Add one entry to `QEMU_STORAGE_PREINSTALLS` with the archive label and the
   destination path `pkg/<package-id>.bex`.
6. Add matching `base_packages` and `autoinstall_packages` entries to
   `device/virtual/qemu/base/aarch64/system_image.prototxt`.
7. If it should launch automatically, mark the process `service: true` and give
   it an explicit `wave` after its dependencies. App-service launches
   storage-preinstalled services in ascending wave order after storage drivers
   have been bound.

Storage-preinstalled D1 drivers should keep bind rules in their manifests;
appd binds those before launching waved storage services. Packages
without an explicit service wave remain installed but are launched only through
debug, lifecycle, or other explicit appd requests.

## Architecture compatibility

Native manifests now require field 20, `architecture: AARCH64` or
`architecture: X86_64`. `app_manifest` stamps the selected guest automatically
before archive signing. Portable packages explicitly use
`app_manifest(..., architecture = "MULTI")`; `MULTI` cannot contain an ELF
executable or shared library. Source manifests remain prototxt.

Rebuild native BEX archives separately for each architecture, including system
images, shared libraries and replacements. Package/version identities and pins
remain unchanged. Signed native packages without a label, or with an incompatible
label, cannot be installed, selected, launched or activated as replacements.
Old durable records remain intact and appear unavailable with a rebuild/reinstall
diagnostic, so one old package does not prevent unrelated packages from loading.
No loader rewrites signed manifests or infers a missing label from an ELF.
## Verified out-of-tree applications

Products import a signed archive with `bexos_prebuilt_app`. Analysis invokes
the BEXARCV2 verifier and accepts only the configured signer key and package
ID, ABI level 1, compatible manifest/ELF architecture, application package
kind, and transplantable services. Package code is never executed during
assembly. The original signed bytes are installed in encrypted `STORAGE` at
`pkg/<package_id>.bex`; the same ID is added to `base_packages` and optionally
to `autoinstall_packages` in the system-image manifest. Duplicate package IDs
or storage destinations fail the build.

The `public_key` input is a prototxt signer record with a 32-byte `key_id` and
Ed25519 `public_key_hex`. Its public key must also be authorized by the target
image's application signing roots. External archives retain their signed
default component configuration; product rewriting or resigning is not part of
SDK v1.

Published application archives should be pinned as content, not executed as
repository rules. For example:

```starlark
http_file(
    name = "vendor_clock",
    urls = ["https://downloads.example/vendor-clock-1.2.3.bex"],
    sha256 = "<verified sha256>",
    downloaded_file_path = "vendor-clock.bex",
)

bexos_prebuilt_app(
    name = "vendor_clock",
    archive = "@vendor_clock//file",
    package_id = "com.example.vendor_clock",
    public_key = "//product/keys:vendor_clock_signer.prototxt",
    autoinstall = True,
)
```
