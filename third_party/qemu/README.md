# ARM secure transition platform

`bazel build //third_party/qemu:secure_aarch64` builds pinned QEMU 11.1.0 with
an additional 256 MiB secure-only transition RAM region at 1 TiB. Its layout
comes from `//secure/platform:qemu_aarch64.prototxt`; Bazel generates C, Rust,
and linker definitions from that one input. The existing low secure firmware
RAM remains available. Normal CPU and device address spaces do not gain the
new region.

The generated layout reserves 16 MiB for the resident owner, two 64 MiB Trusty
banks, a 65 MiB signed-image upload area, and 8 MiB for migration. Generation
rejects missing, overlapping, unaligned or undersized regions.

Stock `virt` provides only 16 MiB of secure RAM; see the
[QEMU platform documentation](https://www.qemu.org/docs/master/system/arm/virt.html).
The probe uses `max` with this exact QEMU pin, rather than claiming CPU state
compatibility across QEMU versions.

The build uses Bazel's LLVM compiler, a pinned source-built Ninja, QEMU's
vendored Python wheels, and a pinned setuptools wheel. Configure runs offline.
The host still supplies Python, pkg-config, the platform SDK, and development
libraries including GLib, libfdt, pixman and libslirp. The current Darwin build
uses the Apple Silicon Homebrew development-library paths. Ubuntu builds require
`libglib2.0-dev`, `libfdt-dev`, `libpixman-1-dev`, and `libslirp-dev`.

`bazel test -c opt //secure/platform:memory_probe_test
--test_tag_filters=requires-qemu --nocache_test_results` builds and runs a
bounded bare-firmware probe. It checks secure access at both ends of transition
RAM, entry into S-EL2, and an abort when NS-EL1 accesses that RAM. It is not a
Trusty replacement test. The runner reaps its QEMU child on exit or timeout.

The maintained ARM product and its QEMU end-to-end tests select this binary
through `BEXOS_QEMU_AARCH64_BINARY`.  TF-A enters the resident S-EL2 owner,
which hosts the active and candidate Trusty images in the generated banks and
keeps the persistent selection region mapped only in the secure address space.
