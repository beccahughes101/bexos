# RFC 0050: User-created applications

- Created: 2026-09-02T20:02:13-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Locally generated applications and PWAs use user-scoped package identities, sandboxed execution, and attenuated capabilities. The design defines isolation, revocation, and cleanup for transient applications.

## Design overview

Locally generated applications and PWAs use the same capability and WASM execution model as other BexOS packages, with user-scoped identities. This gives AI-generated code a managed, isolated lifecycle rather than running it in unconfined scripts or uncontrolled web views.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ CREATION TIER (Local AI Agent / Web PWA Ingest / IDE)                       │
│ • Emits declarative UI (Dioxus/Blitz) + sandboxed WASM or PWA assets       │
│ • Generates manifest with strict `services_consumed` method ordinals        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ 1. Local Compile & Package Assembly
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ LOCAL SIGNING & TRUST AUTHORITY (`keystored` / `packager`)                  │
│ • Holds an opaque hardware-backed User Root Key (Trusty KeyMint on ARM64)   │
│ • Mints ephemeral `user_created:<uid>:<app_id>` x509 cert                  │
│ • Package prefix strictly enforced: `user_created:<uid>:*`                  │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ 2. Signed `.bexapp` Bundle
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ SANDBOX EXECUTION TIER (D2 WASM Runtime / PWA Container)                     │
│ • Isolated VMAR heap with strict memory quotas (e.g., 128MB max)            │
│ • Zero ambient authority: no raw filesystem, no direct device handles      │
│ • Scoped storage: private encrypted namespace at `/data/user/<uid>/apps/...`│
│ • Explicit capability attenuation: channel filters block ungranted ordinals │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Key Security & Isolation Guardrails

### Scope and Identity Sandboxing

* **Prefix Pinned by Kernel/Broker:** `appd` must reject any manifest claiming `user_created:<uid>:` unless signed by that specific user’s local hardware-bound key.
* **Storage Namespace Isolation:** The virtual filesystem (`vfsd`) routes all file requests from this package to `/data/user/<uid>/isolated_apps/<app_id>/`. The app cannot see or map files belonging to the OS, other users, or even other user-created apps unless explicitly brokered through a user-picker capability.

### D2 WASM Execution Constraints

AI-generated code cannot be fully audited by human eyes before running, so the runtime must enforce hard boundaries:

* **No JIT Compilation / Pure WASM Interpretation or AOT Verification:** Run the bytecode inside the hardened D2 WASM runtime (`wasmtime` / `bexos-wasm`).
* **Fuel / Compute Budgeting:** Enforce CPU fuel limits to prevent accidental infinite loops or compute exhaustion.
* **Memory Bounds:** Cap linear memory expansion to a modest default (e.g., 64MB or 128MB) via its private VMAR.

### Strict Permission & Ordinal Attenuation

* The compiler/packager generates the manifest with the exact `services_consumed` and method ordinals needed.
* **Prohibited Platform Capabilities:** User-created apps should be fundamentally barred from requesting sensitive system-level permissions (e.g., raw MMIO, IOMMU mappings, platform background daemons, or administrative IPC ordinals).
* **Consent at Creation Time:** When the AI finishes synthesizing the app, `appd` displays a capability manifest to the user:
> *"This generated app requests: Local Storage (10MB), Network (`api.weather.gov` only), UI Display. Install?"*

## Properties of the `user_created:<uid>:` prefix

* **Eliminates Global Namespace Collisions:** An app generated on one device cannot spoof or shadow store packages like `com.spotify.music` or platform packages like `bexos.platform.netstack`.
* **Multi-User Isolation:** On shared devices (e.g., family tablets or enterprise workstations), User A cannot launch, inspect, or invoke IPC capabilities on User B's synthesized apps.
* **Trivial Revocation & Cleanup:** Removing an AI experiment or transient PWA is an atomic operation: deleting the package directory revokes the capability routing tokens and purges its content-addressed data slice instantly.
