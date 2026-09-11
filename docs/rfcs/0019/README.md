# RFC 0019: Debugd-installed test applications

- Created: 2026-08-28T09:17:29-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

QEMU tests build application artifacts with Bazel, upload them through debugd, and launch them during the test scenario instead of embedding verifier applications in device images.

## Design overview

QEMU e2e test applications are test artifacts, not device image contents. A test
such as `bexos.platform.storage_verify` is built by Bazel, provided to the host
e2e harness as test data, uploaded to the target through debugd, then launched by
the debug service during the scenario.

This keeps device folders focused on hardware and boot configuration. The QEMU
device image owns the encrypted `SYS_STATE` and `STORAGE` partitions plus seed
directories such as `/data`, but it does not directly install verifier packages
like `storage_verify` into `/pkg`.

The current BXD1 app install flow treats a signed `.bex` archive as the only
install artifact:

1. `BeginAppBundleUpload` declares an upload id and total bundle length.
2. `WriteAppBundleChunk` sends archive chunks below the debug-wire frame limit.
3. `CommitAppBundleUpload` forwards the complete bundle to appd through
   the app lifecycle channel.
4. App-service verifies the archive signature and `package.bexmanifest`, imports
   the package into the app registry, asks vfsd to write
   `STORAGE/pkg/<package>.bex`, and returns the package id.
5. `LaunchApp` asks appd to launch a registry-backed package/process
   through the normal ArchiveFS-verified `/pkg` path.

`debugd` does not mutate registry state or make package policy decisions. It
only owns the host transport and buffers upload chunks before proxying the
bundle to appd.

The earlier loose manifest-plus-ELF test flow remains as a narrow unit-test
shim while e2e scenarios move to signed bundle install:

1. `BeginTestAppUpload` declares an upload id, package id, manifest length, and
   ELF length.
2. `WriteTestAppChunk` sends manifest stream chunks and ELF stream chunks below
   the debug-wire frame limit.
3. `CommitTestAppUpload` validates that all bytes arrived.
4. `LaunchTestApp` asks the guest to launch the named package and process with
   the usual e2e namespace, for example `/pkg;/data`.

The host-side e2e library hides chunking from tests. Test authors provide the
compiled manifest and ELF paths and assert guest-visible behavior after launch.
