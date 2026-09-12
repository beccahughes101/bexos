# RFC-0043: Tiered Font Architecture, Zero-Copy VMO Sharing, and Dynamic OCI Discovery

* **Author:** BexOS Graphics & Package Management Working Group
* **Status:** Proposed
* **Target Subsystems:** `fontd`, `pkgd`, `libs/ui`, `sysui`, `userui`, `scened`
* **Applicability:** Dioxus Native Apps, Host Text Shapers (`cosmic-text`/HarfBuzz), Servo Engine

---

## 1. Summary

This RFC specifies the typography subsystem for BexOS. It introduces:

1. **`fontd` (D1 Font Management Daemon):** A centralized system service managing font discovery, sanitization, and shaping query resolution.
2. **Two-Tier Local Font Topology:** An immutable baseline in `/system/data/fonts` alongside isolated per-user font stores in `/data/users/<uid>/fonts`.
3. **Zero-Copy Memory Distribution:** Delivery of parsed font binaries to unprivileged client sandboxes via deduplicated, read-only Virtual Memory Objects (`zx.Handle:VMO`).
4. **Dynamic On-Demand Registry Fetching:** A protocol for resolving missing font families at runtime by fetching TUF-verified OCI font artifacts through `pkgd`.

---

## 2. Motivation

Typography is a frequent source of performance degradation and security vulnerabilities in modern operating systems:

* **Memory Duplication:** When several applications independently load large font collections (e.g., CJK font files spanning 30–50 MB each), memory consumption balloons if each process loads separate disk buffers.
* **Security Surface Area:** Complex font parsing libraries (handling OpenType tables, hinting VMs, and TrueType instructions) have historically suffered from critical memory safety bugs. Unsandboxed client applications parsing arbitrary user-supplied or web-downloaded font files create significant exploit surfaces.
* **System Footprint vs. Completeness:** Modern desktop and enterprise systems require support for thousands of scripts and diverse typography, but bundling every global font family into base system images bloats installation footprints on edge and cloud VM appliances.

BexOS resolves these challenges by centralizing verification and caching within `fontd`, sharing immutable physical pages across processes using microkernel VMOs, and dynamically pulling optional font families over standard OCI package infrastructure.

---

## 3. Detailed Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ CLIENT ADDRESS SPACE (Dioxus Native / Servo / Host UI Runner)               │
│                                                                             │
│  [ Stylo / CSS Style Evaluation ] ──► "font-family: 'Fira Code', monospace" │
│                                                  │                          │
│  [ Text Layout & Shaper (cosmic-text) ]          │ Query                    │
│  • Holds map: `font_id` -> mapped read-only VMAR │                          │
└──────────────────────────────────────────────────┼──────────────────────────┘
                                                   │ FIDL: FontProvider.ResolveFont
                                                   ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `fontd` (D1 Core Font Service)                                              │
│                                                                             │
│  ├── [ Memory-Mapped Index: System BootFS ] (`/system/data/fonts/`)         │
│  │   • Inter, JetBrains Mono, Noto Core fallbacks                           │
│  │                                                                          │
│  ├── [ Per-User Encrypted Storage ] (`/data/users/<uid>/fonts/`)            │
│  │   • User-installed .ttf/.woff2 assets                                    │
│  │                                                                          │
│  └── [ Dynamic OCI Resolver ]                                               │
│      • Resolves unknown font queries against package index                  │
│      • Calls `pkgd.FetchArtifact("pkg.bexos.org/fonts/fira-code")`          │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Shares Deduplicated VMO Handle
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ PHYSICAL RAM (Zero-Copy Shared Pages)                                       │
│                                                                             │
│  [ Read-Only Anonymous / Paged VMO: `FiraCode-Regular.ttf` ]                 │
│         ▲                                   ▲                               │
│         │ Mapped R/O into App A             │ Mapped R/O into App B         │
└─────────┴───────────────────────────────────┴───────────────────────────────┘

```

---

### 3.1 Two-Tier Font Hierarchy

`fontd` categorizes fonts into two local levels of storage before escalating to external discovery:

#### Tier 1: Immutable Platform Baseline (`BootFS` / System Image)

Embedded directly into the system image under `/system/data/fonts/` and indexed at compile time via Bazel:

* **System UI Variable Font:** Canonical sans-serif for shell interfaces and system utilities (e.g., *Inter Variable*).
* **Developer Monospace:** Fixed-width glyphs for dev tools and logging (e.g., *JetBrains Mono*).
* **Core Unicode Fallback:** Minimal *Noto Sans* subsets ensuring error boxes, basic symbols, and core scripts render without tofu (``).

#### Tier 2: Encrypted User Fonts (`/data/users/<uid>/fonts/`)

Users or apps may install local fonts at runtime:

* Files are uploaded through `fontd.InstallUserFont(...)`.
* `fontd` sanitizes the font binary using an isolated parser before saving it to the user's isolated volume.
* User fonts take precedence over platform baseline fonts of the identical family name within that user's session context.

---

### 3.2 Dynamic OCI Font Packaging & Discovery Flow

When an application requests a font that is not present on disk, `fontd` consults the system package registry through `pkgd`.

#### OCI Font Artifact Specification

Fonts are published to standard OCI registries as single-layer or multi-layer artifacts conformant with the OCI Image Spec:

```json
{
  "schemaVersion": 2,
  "mediaType": "application/vnd.oci.image.manifest.v1+json",
  "artifactType": "application/vnd.bexos.font.v1",
  "config": {
    "mediaType": "application/vnd.bexos.font.config.v1+json",
    "digest": "sha256:8f4c20b..."
  },
  "layers": [
    {
      "mediaType": "font/woff2",
      "digest": "sha256:39a1c8f12...",
      "size": 42100,
      "annotations": {
        "bexos.org/font-family": "Fira Code",
        "bexos.org/font-weight": "400",
        "bexos.org/font-style": "normal",
        "bexos.org/font-format": "woff2"
      }
    }
  ],
  "annotations": {
    "bexos.org/license": "OFL-1.1",
    "bexos.org/version": "6.2.0"
  }
}

```

#### On-Demand Resolution Workflow

1. **Resolution Miss:** A document processed by an app requests `font-family: "Fira Code"`.
2. **Lookup Evaluation:** `fontd` confirms that no matching family exists in the active local font index.
3. **Registry Query:** If the client passes `allow_network_fetch: true`, `fontd` requests `pkgd` to check the signed TUF repository index for an artifact providing `Fira Code`.
4. **TUF Verification & Ingestion:** `pkgd` retrieves the layer blob via standard OCI distribution endpoints, authenticates its content digest against the signed TUF `targets.json`, and hands the raw buffer to `fontd`.
5. **Sanitization:** `fontd` validates the table structure (ensuring absence of malformed offsets or table overlaps).
6. **VMO Population & Storage:** The font is stored in a designated on-disk cache (`/data/cache/fonts/`) and retained as an active memory-mapped VMO.
7. **Resolution Return:** The client receives a duplicate read-only handle to the newly populated VMO.

---

### 3.3 Zero-Copy VMO Lifecycle

To prevent redundant memory usage across applications, `fontd` maintains an internal table of initialized VMO handles keyed by content hash:

```rust
struct ActiveFont {
    font_id: u64,
    family: String,
    weight: u16,
    style: FontStyle,
    vmo: zx::Vmo,
    data_len: usize,
}

```

* **Handle Duplication:** When an application calls `ResolveFont`, `fontd` locates the matching `ActiveFont` record and executes:
```rust
let client_vmo = active_font.vmo.duplicate_handle(
    zx::Rights::READ | zx::Rights::MAP | zx::Rights::GET_PROPERTY
)?;

```


* **Address Space Mapping:** The client process receives `client_vmo` over the FIDL response channel and maps it using:
```rust
let mapped_addr = zx_vmar_map(
    client_root_vmar,
    zx::VmOption::PERM_READ,
    0,
    client_vmo.raw_handle(),
    0,
    active_font.data_len,
)?;

```


* **Deduplication:** Multiple client processes mapping the same `font_id` reference identical underlying physical pages. Operating system page tables map the virtual addresses to the same physical memory frames, and pages remain clean and subject to kernel page caching policies.

---

## 4. Interface Definition Language (FIDL)

The core contract resides in `idl/bexos/fonts/provider.fidl`:

```fidl
library bexos.fonts;

using bexos.kernel;

type FontStyle : uint8 {
    NORMAL = 1;
    ITALIC = 2;
    OBLIQUE = 3;
};

type FontFormat : uint8 {
    TRUETYPE = 1;
    OPENTYPE = 2;
    WOFF2 = 3;
};

struct FontDescriptor {
    family_name string:64;
    weight uint16;              // Standard CSS numeric weight (100–900)
    style FontStyle;
    format_preference FontFormat;
};

struct FontHandle {
    font_id uint64;
    /// Read-only handle to the backing physical pages
    data zx.Handle:VMO;
    data_len uint64;
    index_in_collection uint32; // Sub-font index (e.g., for .ttc files)
};

type FallbackScript = string:4; // ISO 15924 script tag (e.g., "Latn", "Hani", "Arab")

@discoverable
protocol FontProvider {
    /// Resolve exact or nearest matching font variant
    ResolveFont(struct {
        query FontDescriptor;
        allow_network_fetch bool;
    }) -> (resource struct {
        font FontHandle;
    }) error bexos.kernel.Status;

    /// Retrieve prioritized fallback list for missing glyph coverage
    GetFallbackList(struct {
        script FallbackScript;
    }) -> (resource struct {
        fonts vector<FontHandle>:8;
    }) error bexos.kernel.Status;

    /// Register a user-provided font binary into the user's isolated store
    InstallUserFont(resource struct {
        font_data zx.Handle:VMO;
        data_len uint64;
    }) -> (struct {
        assigned_id uint64;
    }) error bexos.kernel.Status;
};

```

---

## 5. Security & Isolation Considerations

1. **Client Isolation:** Client applications are never granted direct filesystem access to global font paths. They can only access font data explicitly returned as a read-only VMO by `fontd`.
2. **Handle Rights Restriction:** VMO handles passed to clients are stripped of `WRITE` and `EXECUTE` rights using `zx_handle_replace(..., ZX_RIGHT_READ | ZX_RIGHT_MAP)`. A compromised client process cannot alter font memory for other running processes.
3. **Multi-User Confidentiality:** User-installed fonts from User A (`/data/users/1000/fonts/`) are not visible or resolvable by User B (`/data/users/1001/fonts/`).
4. **Parser Sandboxing:** New fonts coming over the network via OCI or uploaded by a user are validated inside `fontd` using memory-safe Rust font parsers (e.g., `read-fonts`) before being committed to persistent storage or exposed to other processes.

---

## 6. Implementation Roadmap

### Phase 1: Local In-Memory `fontd` Implementation

* Implement `fontd` service in Rust with static indexing of prebundled system fonts (`Inter`, `JetBrains Mono`).
* Implement client-side `FontHandle` caching within the host Dioxus/Stylo runner using `cosmic-text`.
* Verify zero-copy behavior and page sharing across isolated client processes.

### Phase 2: Per-User Font Persistence

* Extend `fontd` to manage per-user font paths under `/data/users/<uid>/fonts`.
* Add font file validation routines and implement `InstallUserFont` FIDL methods.

### Phase 3: OCI Dynamic Discovery via `pkgd`

* Define standard OCI artifact manifests and annotations for `.bex` font packages.
* Implement the network-fetch path connecting `fontd` to `pkgd` using signed TUF verification.
* Integrate font-download permission prompts into `sysui` for interactive user sessions.