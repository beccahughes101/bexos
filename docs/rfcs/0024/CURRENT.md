# RFC 0024: Intent resolution and openers — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0024](README.md)

## Implementation summary

Appd resolves manifest-declared intents and URLs using caller scope, cached domain verification, and stored user defaults.

## Implemented behavior

- The opener registry indexes process handlers and matches MIME types, schemes, and verified domains. A unique/default handler can be resolved under system or user scope.
- Appd exposes the Opener contract and retains caller-bound opener state through migration. Persistence libraries store registrations/defaults; domain refresh updates cached verification.
- The user desktop uses a UID-bound opener to launch installed apps.

## Gaps and deviations

- Ambiguous matches return PROMPT_PENDING_USER; there is no completed handler-picker and consent interaction.
- Domain verification depends on cached policy. The public well-known refresh path currently uses UnavailableFetcher, so it is not a live production HTTPS association service.
- Intent filtering and shell launch do not implement the RFC’s entire future app/browser ecosystem or prove every cross-user/deep-link workflow.

## Sources and validation

Implementation and contract evidence: [lib/opener_store/src/lib.rs](../../../lib/opener_store/src/lib.rs), [services/appd/src/opener.rs](../../../services/appd/src/opener.rs), [services/appd/src/manager.rs](../../../services/appd/src/manager.rs), [idl/bexos/app/opener.fidl](../../../idl/bexos/app/opener.fidl), [apps/userui/src/desktop.rs](../../../apps/userui/src/desktop.rs).

Relevant test sources and Bazel targets: [lib/opener_store/BUILD.bazel](../../../lib/opener_store/BUILD.bazel), [services/appd/tests/appd_tests.rs](../../../services/appd/tests/appd_tests.rs).

Detailed guides and previously recorded validation: [appd userspace](../../appd-userspace.md), [sysui](../../sysui.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
