# RFC 0005: Provider-neutral TEE integration

- Created: 2026-08-26T17:31:23-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

TEE providers share a capability-gated kernel boundary and a versioned D1 driver ABI. Provider-neutral orchestration manages trusted-app packages, sessions, and replacement.

## Design overview

[RFC 0051](../0051/README.md) is the canonical detailed design for the current
BexOS TEE direction. This file defines the stable split that every provider
must obey.

## Current D0/D1 boundary

BexOS D0 exposes a generic, capability-gated Secure Monitor call surface and
contiguous-memory primitives. The kernel accepts and returns `x0..x7` through
`bexos.kernel.SecureMonitor.Call`; it does not parse Trusty, OP-TEE,
GlobalPlatform, TIPC, apploader, or trusted-app messages.

Only the signed, configured `bexos.service.teed` process receives
`AUTH_SECURE_MONITOR`. Package names do not grant memory-management exceptions.
Callers that need shared memory create, map, pin, and unpin VMOs explicitly.

Provider-specific logic lives in D1 behind the versioned external C ABI shipped
as `libbexos_tee_driver.so`:

- `//lib/tee_driver_client` defines ABI version 1 and the
  `bexos_tee_driver_*` symbol prefix.
- `bexos.lib.tee_driver.trusty` implements the real Trusty provider for the
  QEMU ARM64 Trusty product and fails closed on missing symbols, ABI mismatch,
  probe failure, or transport failure.
- `bexos.lib.tee_driver.software` implements the same ABI for explicit
  software-emulated products and tests.

The real product must never silently fall back to software emulation.

## `teed`

`teed` is provider-neutral orchestration. It loads exactly one ABI-compatible
driver selected by its signed manifest, owns public session IDs and app/catalog
metadata, and migrates that public state across heart transplant. During a
`teed` or driver transplant, pending driver operations are failed as
unavailable, the new library is bound and probed, package-managed apps are
reloaded, and catalogued endpoints are reconnected. Secure-app private volatile
state is not preserved.

`bexos.tee.TeeManager` keeps ordinals 1-9 for the legacy developer surface and
adds package-aware listing/status, `OpenSessionByEndpoint(package_id,
service_port)`, and appd-only activation/deactivation/query methods for durable
trusted-app packages.

## Trusted apps

Trusted apps are ordinary signed BexOS packages with
`PackageKind.TRUSTED_APP`. Their manifest carries `TrustedAppInfo`: provider,
16-byte UUID, secure monotonic version, embedded upstream Trusty app payload
path, allowed service-port names, and protected/uninstallable policy.

`appd` owns durable activation. It rejects unsafe manifests, installs updates
probationarily, asks `teed` to unload/load/probe the candidate, marks healthy
only after success, and rolls back to the previous archive on failure. Updates
and uninstalls are rejected while sessions for the target trusted app are
active. Legacy raw install/uninstall commands remain developer-only,
non-durable operations and cannot replace package-managed or built-in apps.

## Trusty core updates

For QEMU ARM64, Trusty/LK core updates are separate from TF-A/platform updates.
The long-term mechanism is the signed `BEXTEEAB` secure A/B capsule described
in [RFC 0051](../0051/README.md): stage inactive bank, live-switch through TF-A,
require the built-in orchestrator to confirm health, roll back on timeout or
transport failure, then rebuild transport and reconnect public sessions
statelessly.

Current build/unit verification covers the BexOS ABI, package-management path,
QEMU Trusty firmware build, and secure-state models. The QEMU e2e suite is not
claimed for live Trusty core switching.

## Future provider designs retained

Future hardware work may add additional ABI implementations for AMD PSP,
Intel SGX/TDX, Nitro, or an OP-TEE compatibility provider. Those providers must
remain D1 libraries and must not move provider wire formats into D0.
