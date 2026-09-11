# RFC 0060: System and user UI

- Created: 2026-09-09T07:40:12-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

A persistent system UI owns trusted display and session boundaries; a per-session user UI hosts the user workspace. The design retains the initial single-display milestone and the broader multi-user and multi-monitor architecture.

## Basic implementation milestone

The initial implementation uses separately packaged Dioxus WASM services and native
Flatland rendering. It targets one display and one authenticated user session:
login, lock/unlock, logout, an installed-app launcher, and floating windows.
The exact implemented contracts, limitations, CLI examples, and validation are in
[the current SysUI document](../../sysui.md).

The stable `bexos.platform.appd` preferences schema owns two package selectors.
`sysui_package` is system-only and defaults to `bexos.app.sysui`;
`userui_package` is user-editable and defaults to `bexos.app.userui`.
The latter resolves manifest defaults, system defaults, and per-UID overrides
through prefsd only after usersd authenticates and unlocks the user's preferences.
Operator locks retain their normal precedence. System selection is captured at
boot, user selection at fresh login. Invalid packages fall back to the bundled
package with a diagnostic, without rewriting the saved selector.

A shell entrypoint declares `SHELL_SYSTEM` or `SHELL_USER` in its prototxt process
manifest, runs as a service, and supports heart transplant. Appd grants authority
only to the selected entrypoint with its bound package, process, UID, and session.
Production scened starts in shell composition mode: SysUI is the display root,
UserUI is its child, and ordinary application views must be embedded in that
user's desktop. Legacy flat composition is an explicit graphics-fixture option.
The implemented FIDL uses 64-bit UIDs and channel-backed view references. Appd
implements the authenticated session broker; UserShell callbacks deliver display
attachment and state changes. This is the first milestone of the longer-term
architecture below, whose multi-display, overlays, widgets, and fast-switching
proposals remain future work.

The milestone's concrete gaps are tracked in
[the current SysUI document](../../sysui.md#known-gaps). The shipping product
still has one active session, fixed view and process-table limits,
package-scoped window closing, CLI-only account and shell selection, and
ASCII-only shell text. The architecture below describes intended behavior beyond
those limits; its presence does not imply that those features ship today.

## Long-term architecture

Structuring the shell as two distinct Dioxus native components—a root **System UI (`sysui`)** and an ephemeral, per-session **User UI (`userui`)**—aligns process isolation with capability boundaries.

This separation prevents untrusted user-session code or compromised widgets from tampering with system-critical indicators, biometric authentication, or login security prompts.

### Process Topology & View Hierarchy

The Flatland-style `scened` compositor organizes the display hierarchy as a capability-gated tree:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ `scened` (Hardware Display Server / Compositor)                             │
│ Owns physical display scanout buffers                                       │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Grants Root View Tokens per display
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `sysui` (System Scope / D1 Trusted Platform Component)                      │
│ • Runs continuously at system scope (UID 0 / System Group)                  │
│ • Renders: Global lock screen, login prompt, volume/brightness overlays,    │
│   emergency power menu, secure credential popups                            │
│ • Holds parent View Tokens for each physical display                        │
└───────────────────┬─────────────────────────────────────┬───────────────────┘
                    │ Creates Viewport Token              │ Creates Viewport Token
                    ▼ (User A Active)                     ▼ (User B Background)
┌──────────────────────────────────────┐ ┌────────────────────────────────────┐
│ `userui` (User A Session / D2 Scope) │ │ `userui` (User B Session / D2 Scope) │
│ • Sandboxed under UID 1000           │ │ • Sandboxed under UID 1001           │
│ • Taskbar / Dock / App Launcher      │ │ • Suspended or Background state      │
│ • Window Management & Tiling/Floating│ │ • Memory limits throttled            │
│ • Third-party Dioxus Widgets         │ │                                    │
│ ──► Embeds child app view tokens     │ │                                    │
└──────────────────────────────────────┘ └────────────────────────────────────┘

```

### Core Security & Architectural Boundaries

#### Tamper-Proof Overlay Guarantees

Because `sysui` owns the root view of every display and mounts `userui` inside a child viewport:

* **No Spoofed System Prompts:** A malicious application or widget inside `userui` cannot draw over system-level overlays. The volume indicator, low-battery warning, and hardware credential confirmation prompts are rendered by `sysui` on a compositor layer strictly above the user session tree.
* **Secure Input Interception:** When the lock screen triggers or a privilege escalation dialog (e.g., biometric prompt, Trusty authentication) appears, `sysui` claims exclusive input focus from `scened`. Keyboard and touch events never reach the `userui` window tree.

#### Session Lifecycle & Multi-User Switching

`sysui` coordinates with `appd` to manage the lifecycle of user environments:

* **Login:** The user authenticates against Trusty via `sysui`. Upon success, `sysui` requests `appd` to launch (or resume) that user's session.
* **View Attachment:** `sysui` creates a child `ViewportToken` via `scened` for each active monitor and transfers the matching `ViewToken` handles to the user's `userui` process over FIDL.
* **Fast User Switching:** When switching from User A to User B:
1. `sysui` detach-calls the root view of User A, making it instantly invisible.
2. `sysui` attaches the `ViewportToken` of User B.
3. `appd` deprioritizes User A's resource group (freezing GPU rendering loops, throttling CPU scheduling shares) while keeping session state in memory.

* **Session Lock:** Locking the session detaches or obscures the child user viewport and moves focus back to `sysui`'s PIN/password input field.

#### Sandboxing the Widget System

Allowing `userui` to be extended with custom widgets introduces security risks if uncontained:

* **Widgets Run in WASM:** Third-party widgets should execute inside isolated WASM instances (e.g., via Wasmtime) rather than linking as native shared libraries directly into `userui`.
* **Data-Only or Display-List IPC:** Widgets export either raw state models to Dioxus or emit a restricted 2D layout delta. A misbehaving widget cannot crash the window manager or read unauthorized user files.

### Proposed FIDL Protocol: `bexos.shell.session`

```fidl
library bexos.shell.session;

using bexos.kernel;
using bexos.scened;

enum SessionState : uint8 {
    ACTIVE_FOCUSED = 1;
    BACKGROUND_SUSPENDED = 2;
    LOCKED = 3;
};

/// Implemented by `sysui` and called by `appd` / Auth services
@discoverable
protocol SessionManager {
    /// Present login screen and clear active user viewports
    ShowLoginScreen() -> (struct { status bexos.kernel.Status; });

    /// Activate a specific logged-in user session
    SwitchToSession(struct {
        user_id uint64;
    }) -> (struct { status bexos.kernel.Status; });

    /// Lock current session and switch to lock screen overlay
    LockCurrentSession() -> ();
};

/// Implemented by the per-user `userui` process
protocol UserShell {
    /// Hand off compositor display tokens when a display is added or session starts
    AttachDisplayView(resource struct {
        display_id uint64;
        view_token bexos.scened.ViewToken;
    }) -> (struct { status bexos.kernel.Status; });

    /// Notify user shell of lifecycle changes (suspend rendering, lock animations)
    SetSessionState(struct {
        state SessionState;
    }) -> ();
};

```

### Handling Multi-Monitor Setups

When a user plugs in a secondary display (e.g., via HDMI or DisplayPort):

1. The display bus driver registers the new node with `scened`.
2. `scened` allocates a new display surface and notifies `sysui`.
3. `sysui` instantiates a secondary top-level viewport and passes an `AttachDisplayView` token to the active session's `userui`.
4. `userui` mounts a new Dioxus rendering tree configured for the secondary display (e.g., secondary taskbar, wallpaper, and empty workspace).
