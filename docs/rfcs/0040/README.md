# RFC 0040: Compositor, system shell, and launcher

- Created: 2026-08-30T22:22:36-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

The shell design separates the hardware compositor, unified system chrome, and a pluggable launcher. Role binding, capability routing, and lock-screen rules define their interaction.

## Design overview

The user interface has three tiers: the **hardware compositor (`scened`)**, a unified **system shell (`shell.bex`)**, and a pluggable **launcher (`launcher.bex`)**.

This hybrid design avoids the monolithic brittleness of ChromeOS Ash (which combined the browser, window manager, and UI in one process) while preventing the fragmented gesture-coordinate synchronization issues historically seen between Android's `SystemUI` and `Launcher3`.

## Layered Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. SYSTEM COMPOSITOR (`scened` - D1 Userspace Daemon)                       │
│    • Direct DRM / Vulkan / VirtIO-GPU scanout                               │
│    • Global input router (`inputd` consumer) & window layer z-ordering      │
│    • Enforces secure display zones (Lock screen / Trusted Permission Dialogs)│
│    • Contains ZERO layout widgets, text parsers, or UI theme assets         │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ FIDL (`bexos.ui.scened.ShellRole`)
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. SYSTEM SHELL (`com.bexos.shell:sysui` - Dioxus Native WASM App)          │
│    • Persistent System Shell running in an isolated user process            │
│    • Renders: Top status bar, quick settings, notifications, wallpaper      │
│    • Manages Lock Screen / Credential Unlock UI (talks to `keychaind`)      │
│    • Owns the gesture animation timeline (e.g. swipe down for Quick Settings│
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Launch Intents / App Grid Data
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. PLUGGABLE LAUNCHER (`com.bexos.launcher:home` / Third-Party Launchers)  │
│    • App grid, home screen widgets, dock, global search                     │
│    • Communicates with `appd` to list installed packages and metadata       │
│    • User-replaceable via standard Opener default handler                   │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Rationale for the separation

* **`scened` is purely functional (Wayland / Window Manager style):** It only deals with surfaces, VMOs, clip rectangles, and input routing. If a UI bug crashes the top bar or launcher, `scened` stays alive and restarts them without losing open application states.
* **Unified System Chrome in `shell.bex`:** The top bar, control center, lock screen, and wallpaper belong together in one Dioxus process. When a user drags down from the top bar to open the quick settings, the status bar icons morph into full tile cards without cross-process IPC synchronization or gesture-passing glitches.
* **Pluggable Launcher:** Separating the home app drawer and desktop allows users or OEM distributions to swap the launcher (e.g., standard phone grid vs. desktop-style taskbar launcher) without touching the trusted lock screen or system security dialogs.

## Layer Responsibilities

```
                                  [ Z-ORDER LAYERS IN SCENED ]
  Top
   ▲   ┌─────────────────────────────────────────────────────────────────────┐
   │   │ Layer 4: Trusted Dialogs (`policyd` consent prompts, power menu)    │ ◄── Owned by `sysui`
   │   ├─────────────────────────────────────────────────────────────────────┤
   │   │ Layer 3: Lock Screen / Auth Shade (`keychaind` PIN unlock)          │ ◄── Owned by `sysui`
   │   ├─────────────────────────────────────────────────────────────────────┤
   │   │ Layer 2: Status Bar / Quick Settings / Notification Shade          │ ◄── Owned by `sysui`
   │   ├─────────────────────────────────────────────────────────────────────┤
   │   │ Layer 1: Running Application Windows (Dioxus Apps, Terminal, etc.)  │ ◄── Managed by `scened`
   │   ├─────────────────────────────────────────────────────────────────────┤
   │   │ Layer 0: Home Screen / App Grid & Live Wallpaper                    │ ◄── Owned by `launcher`
   ▼   └─────────────────────────────────────────────────────────────────────┘
  Bottom

```

## Shell Role Binding FIDL (`bexos.ui.scened`)

`scened` uses capability-restricted roles so only verified platform components can register background wallpapers or top bars:

```fidl
library bexos.ui.scened;

using bexos.kernel;

type ShellRole = strict enum : uint8 {
    WALLPAPER        = 1;
    STATUS_BAR       = 2;
    OVERLAY_PANEL    = 3; // Quick settings / Notifications
    LOCK_SCREEN      = 4;
    APPLICATION      = 5;
};

@discoverable
protocol WindowManager {
    /// Privileged entrypoint (Requires `bexos.permission.SYSTEM_SHELL`)
    RegisterShellSurface(resource struct {
        role ShellRole;
        surface_channel server_end:Surface;
    }) -> (struct {
        status bexos.kernel.Status;
    });

    /// Inform the compositor of system inset sizes (e.g. status bar height)
    SetSystemInsets(struct {
        top uint32;
        bottom uint32;
        left uint32;
        right uint32;
    }) -> (struct {
        status bexos.kernel.Status;
    });
};

```

## Lock Screen Security & Vault State Handling

### Before Vault Unlock (Cold Boot)

* `scened` renders only the `LOCK_SCREEN` surface from `sysui`.
* User enters their PIN/Passkey $\longrightarrow$ `sysui` passes the token to `keychaind`/`teed` via FIDL to derive the encryption key for `/vault/user_<uid>`.
* Applications and the user's `launcher` process **do not spawn** until `vfsd` successfully mounts the decrypted vault.

### After Vault Unlock

* `sysui` signals `scened` that authentication succeeded.
* `scened` animates the lock screen away, unmasking the `launcher` and foreground applications.
