# RFC 0034: Application and system extensions

- Created: 2026-08-30T11:10:51-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Sandboxed WASM packages expose extension points through appd-brokered capabilities. In-process and out-of-process execution models support application plugins and system UI extensions.

## Design overview

Application and OS extensions are sandboxed WASM packages that export FIDL extension hooks, called **Extension Points**. `appd` brokers access through capability handles.

Rather than relying on unconstrained, in-process code injection (which causes instability and security exploits), an extension in BexOS operates as an **Isolated Capability Filter/Plugin**.

## The Extension Architecture

```
+-------------------------------------------------------------------------+
|                  Host Application / OS Surface (e.g. Browser)           |
|                                                                         |
|  1. Declares Extension Point: "bexos.web.WebRequestFilter"              |
|  2. Calls appd.GetExtensions("bexos.web.WebRequestFilter")       |
+-------------------------------------------------------------------------+
                                     |
                                     v (FIDL Pipeline)
+------------------------------------+------------------------------------+
|  Extension A (WASM / Isolated)     |  Extension B (WASM / Isolated)     |
|  - "uBlock Origin" (com.ublock)    |  - "Password Vault" (com.1password)|
|  - Capabilities: Intercept URLs    |  - Capabilities: AutoFill Form     |
+------------------------------------+------------------------------------+

```

## Declaring Extension Points (Host App or OS)

Any app or OS service can declare **Extension Points** in its manifest, specifying the required FIDL protocol that plugins must implement and the CEL permissions required to hook into it:

```protobuf
// Manifest for Browser App (com.google:chrome)
package_name: "com.google:chrome"

extension_points: [
  {
    id: "bexos.web.WebRequestFilter"
    protocol: "bexos.web/RequestFilterProtocol"
    description: "Allows plugins to inspect, modify, or block network requests"

    // Limits which extensions can attach via CEL
    attach_policy: "plugin.permissions.contains('INTERCEPT_NETWORK')"
  },
  {
    id: "bexos.web.BrowserActionUI"
    protocol: "bexos.web/ToolbarButtonProtocol"
  }
]

```

## Declaring an Extension Package

An extension is packaged as a standard `.bexapp` bundle targeting the **WASM runner** with an `extension_binding` entry:

```protobuf
// Manifest for uBlock Origin (com.ublock:origin)
package_name: "com.ublock:origin"

processes {
  name: "filter_engine"
  runner: WASM
  permissions: ["INTERCEPT_NETWORK"]
}

extensions: [
  {
    target_extension_point: "bexos.web.WebRequestFilter"
    target_package: "com.google:chrome"  # Or wildcard '*' for system-wide hooks
    handler_protocol: "bexos.web/RequestFilterProtocol"
  }
]

```

## Execution Topology: In-Process vs. Out-of-Process

Depending on performance requirements, `appd` and the host app coordinate extensions in two ways:

| Mode | Execution Model | Performance | Security / Fault Boundary |
| --- | --- | --- | --- |
| **Out-of-Process (Default)** | Extension runs in a dedicated WASM process container; communicates via zero-copy FIDL buffers. | Microsecond IPC | **Total crash isolation.** If the extension panics or hangs, the host app continues running uninterrupted. |
| **In-Process Sandboxed WASM** | Host embeds the WASM bytecode in a local sub-memory boundary (using Extism/Wasmtime memory limits). | Direct function call speed (Nanoseconds) | Sandbox memory safety guaranteed by WASM; no access to host memory outside exported buffers. |

## OS-Level Extensibility (Extending Core System Shell)

The BexOS Compositor and System Shell use the exact same model:

* **Status Bar / Quick Settings Plugins:** Third-party apps (e.g., Spotify) register for the `bexos.system.QuickSettingsTile` extension point.
* **Virtual Input / Keyboard:** Alternative keyboards or IME systems register for `bexos.system.InputMethodEngine`.
* **Context Menu Actions:** Apps register `bexos.system.ShareTarget` or `bexos.system.FileContextMenuAction` filtered by MIME type metadata.

Because all extension discovery and capability minting are handled by `appd` using CEL and FIDL contracts, extending an app or extending the entire OS uses **one unified, memory-safe, architecture-agnostic mechanism**.
