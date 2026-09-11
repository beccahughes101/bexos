# RFC 0024: Intent resolution and openers

- Created: 2026-08-29T10:39:32-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Appd resolves intents and deep links using manifest filters, domain associations, per-user defaults, and capability routing. Ambiguity handling and future picker behavior remain explicit.

## Design overview

This design integrates intent resolution, deep linking, default handlers, and capability routing into `appd`'s manifest and process management model.

Implementation note: the current vertical slice implements manifest intent
filters, generated `bexos.app.opener.Opener` FIDL, in-memory system/user
resolution, user defaults, host redb persistence coverage, appd runtime
serving, migratable opener bindings, and appd reload-time domain-association
cache updates. The current slice intentionally does not perform production
network-backed `.well-known` fetches and does not include the
future picker UI; ambiguous matches return `PROMPT_PENDING_USER`.

## Manifest Schema: Declaring Intent Filters & Handlers

Apps declare their supported URL schemes, HTTPS App Links, MIME types, and exposed default interfaces in the manifest:

```protobuf
syntax = "proto3";

package bexos.manifest;

message IntentFilter {
  // Custom schemes (e.g. ["monzo", "slack"])
  repeated string schemes = 1;

  // Domain verification for universal links (e.g. ["monzo.com"])
  repeated string domains = 2;

  // MIME types for file handlers (e.g. ["application/pdf", "image/*"])
  repeated string mime_types = 3;

  // Implemented standardized FIDL interfaces (e.g. ["bexos.ui.WebViewEngine"])
  repeated string provides_interfaces = 4;
}

message ProcessDefinition {
  string name = 1;
  string runner = 2;
  repeated IntentFilter handles = 3;
}

```

## Opener Registration & the Dual `redb` Tables

When an app is installed or soft-activated, `appd` populates the two `redb` registry stores:

```
┌─────────────────────────────────────────────────────────────────────────┐
│ SYSTEM OPENER STORE (/system/state/opener.redb)                         │
├─────────────────────────────────────────────────────────────────────────┤
│ Tables:                                                                 │
│ • `system_url_handlers`:     "https://monzo.com" ──► ["com.monzo:app"]  │
│ • `system_mime_handlers`:    "application/pdf"   ──► ["com.bexos:pdf"]  │
│ • `system_interface_impls`:  "bexos.ui.WebView"  ──► ["com.bexos:webkit"]│
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────┐
│ USER OPENER STORE (/vault/user_<uid>/opener.redb)                       │
├─────────────────────────────────────────────────────────────────────────┤
│ Tables:                                                                 │
│ • `user_url_handlers`:       "customscheme://"   ──► ["user.app:client"]│
│ • `user_mime_handlers`:      "application/pdf"   ──► ["user.app:reader"]│
│ • `user_interface_impls`:    "bexos.ui.WebView"  ──► ["com.google:chrome│
│ • `user_defaults`:           "mime:application/pdf" ──► "user.app:reader│
│                              "interface:bexos.ui.WebView" ──► "com.googl│
└─────────────────────────────────────────────────────────────────────────┘

```

## Resolution Scope Rules

When an app invokes a resolution method on `appd`, `appd` checks the caller context:

### Caller is a System Process (Tier 0 / System Daemon)

* Queries **only** the `/system/state/opener.redb` store.
* Ensures critical system subsystems never inadvertently delegate execution or sensitive files to untrusted user-level apps.

### Caller is a User Session Process (Tier 2 / Consumer App)

* Queries `/vault/user_<uid>/opener.redb` first.
* Falls back to `/system/state/opener.redb` if no user override or soft-installed handler exists.

## Handler Resolution & Disambiguation Flow

```
[ Caller invokes OpenFile(file_handle, "application/pdf") ]
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ appd Resolution Engine                                                  │
│                                                                         │
│ 1. Check user defaults table: `user_defaults["mime:application/pdf"]`    │
│    ├── Default Set? ──► Directly launch default handler app             │
│    │                                                                    │
│    └── No Default Set?                                                  │
│          ├── 1 Candidate: Launch candidate app                          │
│          └── > 1 Candidates:                                            │
│                • Spawn System Intent Picker Prompt UI                   │
│                • User selects app (with "Always / Just Once" toggle)    │
│                • If "Always" checked: Write to `user_defaults` table    │
└───────────────────────────┬─────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ Process Launch & Direct Capability Handover                             │
│ • Launch target handler process (or reuse running instance)             │
│ • Transfer `file_handle` or URL payload directly over startup channel   │
└─────────────────────────────────────────────────────────────────────────┘

```

* **Domain Verification Security:** For `https://` domain handlers (e.g., [https://monzo.com](https://monzo.com)), `appd` validates the app's signing certificate against the domain's `.well-known/bexos-manifest.json` before registering it as an automatic default URL handler, preventing link-hijacking attacks.

## `bexos.app.Opener` FIDL Protocol

```fidl
library bexos.app;

using bexos.kernel;
using bexos.vfs;

type OpenTarget = strict union {
    1: default_process bool;
    2: specific_process string:64;
};

type OpenResult = strict enum : uint8 {
    SUCCESS                 = 1;
    PROMPT_PENDING_USER     = 2;
    NO_HANDLER_REGISTERED   = 3;
    ACCESS_DENIED           = 4;
};

@discoverable
protocol Opener {
    /// Open a URL scheme (e.g. "monzo://pay", "https://waymo.com/ride")
    OpenUrl(struct {
        url string:2048;
        target OpenTarget;
    }) -> (struct {
        status bexos.kernel.Status;
        result OpenResult;
    });

    /// Open a file by handing over a direct VFS capability
    OpenFile(resource struct {
        file handle:CHANNEL; // Implements bexos.vfs.File
        mime_type string:128;
        target OpenTarget;
    }) -> (struct {
        status bexos.kernel.Status;
        result OpenResult;
    });

    /// Open an app directly by package identifier
    OpenApp(struct {
        package_name string:128;
        target OpenTarget;
        arguments vector<string:256>:16;
    }) -> (struct {
        status bexos.kernel.Status;
        result OpenResult;
    });

    /// Discover and bind the preferred implementation of a system interface
    GetPreferredInterface(resource struct {
        interface_name string:128;
        server_channel handle:CHANNEL; // Bound to preferred implementation
    }) -> (struct {
        status bexos.kernel.Status;
        selected_package string:128;
    });

    /// Set an explicit default handler for a MIME type, scheme, or interface
    SetUserDefault(struct {
        target_key string:128; // e.g., "mime:application/pdf", "scheme:mailto"
        package_name string:128;
    }) -> (struct {
        status bexos.kernel.Status;
    });
};

```

## Design properties

* **Preserves Capability Security:** `OpenFile` transfers an unforgeable `bexos.vfs.File` channel handle to the recipient app. The receiving app does not need broad `/data` or storage permissions—it only receives authority over the single file passed via the opener.

* **Pluggable System Implementations:** `GetPreferredInterface("bexos.ui.WebViewEngine")` allows replacing major system components (like switching the default web engine or PDF viewer) with zero re-compilation of consuming apps.
* **Hermetic Multi-User Isolation:** User defaults and custom handlers stay isolated in `/vault/user_<uid>/opener.redb` and automatically clean up on user profile deletion.
