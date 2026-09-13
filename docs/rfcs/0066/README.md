# RFC-0066: Two-Tier Localization Architecture, Shared CLDR VMOs, and Reactive Fluent String Resolution

* **Author:** BexOS Internationalization & Core Frameworks Working Group
* **Status:** Proposed
* **Target Subsystems:** `localed`, `prefsd`, `pkgd`, `appd`, `libs/ui`, `lib/userspace/i18n`
* **Applicability:** Dioxus Native WASM/Native Apps, Host UI Runners, System Shell (`sysui`/`userui`)

---

## 1. Summary

This RFC establishes the internationalization (i18n) and localization (l10n) architecture for BexOS. It introduces:

1. **Decoupled Formatting vs. Translation:** Separation of cultural formatting conventions (Unicode CLDR data) from application-specific message translation catalogs (Mozilla Fluent FTL).
2. **`localed` (D1 Core Locale Service):** A platform daemon maintaining the global Unicode CLDR data repository compiled into a single, memory-mappable binary image (`cldr.bexloc`) distributed to client runtimes via read-only Virtual Memory Objects (`zx.Handle:VMO`).
3. **Reactive In-Memory Translation:** Compilation of Fluent message bundles at Bazel build-time into static application assets, bound directly into Dioxus reactive signal trees to allow instant language switching without process restarts.
4. **Dynamic Supplementary Language Packs:** On-demand fetching of extended locale dictionaries, CJK collation tables, and supplementary CLDR subsets via signed OCI/TUF packages managed by `pkgd`.

---

## 2. Motivation

Traditional Unix and Linux localization strategies suffer from structural deficiencies that conflict with microkernel isolation and performance targets:

* **Memory Duplication of Formatting Tables:** Dynamically linking traditional monolithic C libraries (like standard ICU4C) duplicates tens of megabytes of compiled locale tables across each sandboxed process.
* **Fragile Formatting Systems:** Legacy formats such as GNU `gettext` (`.po`/`.mo`) lack expressive support for complex grammatical rules, including asymmetric plural categories (e.g., Slavic plural forms), gender inflection, and terms dependent on UI state.
* **Ambient Filesystem Access:** Scanning directory paths like `/usr/share/locale/` breaches capability-based sandbox boundaries and introduces file I/O latency on initial application paint.
* **Disruptive Locale Transitions:** Changing system display languages on traditional desktop platforms frequently demands logging out of the desktop session or restarting running applications to clear localized string caches.

BexOS addresses these issues by treating Unicode CLDR tables as a shared kernel-backed memory page and structuring UI text around reactive, compiled Fluent catalogs.

---

## 3. Detailed Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ APPLICATION RUNTIME (Dioxus Native App Sandbox)                             │
│                                                                             │
│  [ Dioxus Reactive UI Tree ]                                                │
│  • Reads: `t!("save-button")` via reactive `use_locale()` signal context   │
│  • Local Catalog: Bound to bundled `strings.bexres` (Mozilla Fluent)        │
│                                                                             │
│  [ ICU4X ZeroCopyDataProvider ]                                             │
│  • Reads: Mapped pointer to `cldr.bexloc`                                   │
│  • Formats: Numbers, currencies, dates, list conjunctions                   │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Inherited VMO Handle / FIDL Updates
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `localed` (D1 Core Localization Daemon)                                     │
│                                                                             │
│  ├── [ Active CLDR Engine ]                                                 │
│  │   • Manages system base: `/system/data/locale/cldr.bexloc`               │
│  │   • Mounts supplementary tables from `/data/cache/locales/`              │
│  │                                                                          │
│  └── [ Locale Broadcaster ] ──► Pushes locale chain updates to active apps  │
└───────────────────┬─────────────────────────────────────┬───────────────────┘
                    │ Registry Pull                       │ User Preferences
                    ▼                                     ▼
┌──────────────────────────────────────┐ ┌────────────────────────────────────┐
│ `pkgd` (OCI/TUF Package Daemon)      │ │ `prefsd` (Configuration Store)     │
│ • Fetches `bexos.locale.<lang>`      │ │ • Holds user preference chain:     │
│ • Validates cryptographic signatures │ │   `["es-MX", "es", "en-US"]`       │
└──────────────────────────────────────┘ └────────────────────────────────────┘

```

---

### 3.1 Two-Tier Structural Division

The localization system strictly bifurcates algorithmic data formatting from localized narrative strings:

#### Tier 1: Canonical Formatting Engine (Unicode CLDR + ICU4X)

Handled globally by `localed`. It encapsulates:

* Date, time, and calendar formatting (e.g., Gregorian vs. Buddhist, 12h vs. 24h).
* Number punctuation (e.g., `1,234.56` in `en-US` versus `1.234,56` in `de-DE`).
* Currency symbols, position rules, and accounting formats.
* Script directionality (LTR vs. RTL layout signals for Taffy).
* Grammatical plural category algorithms (`zero`, `one`, `two`, `few`, `many`, `other`).

This data is sourced directly from upstream Unicode CLDR, serialized using the memory-safe **ICU4X** data provider model into a single zero-copy binary blob: `cldr.bexloc`.

#### Tier 2: Narrative UI Messages (Mozilla Fluent)

Handled process-locally by applications. It encapsulates:

* Button labels, error text, dialogs, and titles.
* Terminology references and brand names.
* Variable text interpolations sensitive to plural counts or grammatical genders.

---

### 3.2 Build-Time Tooling & Artifact Format

#### Fluent Catalog Compilation

Application packages place message definitions within standard locale subdirectories:

```
//apps/system_settings/
├── BUILD.bazel
├── src/
└── locales/
    ├── en-US/
    │   └── settings.ftl
    ├── es/
    │   └── settings.ftl
    └── ja/
        └── settings.ftl

```

A specialized Bazel rule (`bexos_locale_bundle`) compiles the `.ftl` sources into an optimized binary message catalog (`strings.bexres`):

```python
load("//build/rules:locale.bzl", "bexos_locale_bundle")

bexos_locale_bundle(
    name = "settings_locales",
    srcs = glob(["locales/**/*.ftl"]),
    default_locale = "en-US",
)

```

The output `strings.bexres` file uses flat table structures enabling constant-time string offset lookups without requiring runtime AST parsing of raw text files.

---

### 3.3 Zero-Copy CLDR Memory Distribution

To prevent redundant allocations, applications never allocate local copies of the CLDR tables:

1. **Service Initialization:** During boot, `localed` memory-maps `/system/data/locale/cldr.bexloc` into an internal kernel VMO.
2. **Process Spawn Handoff:**
When `appd` prepares a child process sandbox:
* It requests a duplicate handle to the active CLDR VMO from `localed`.
* Permissions are strictly restricted using `zx_handle_replace(..., ZX_RIGHT_READ | ZX_RIGHT_MAP)`.
* The handle is populated in the process startup table under handle tag `PA_LOCALE_DATA_VMO`.


3. **Client-Side ICU4X Hydration:**
The application startup harness initializes an `ICU4X` instance directly from the mapped memory slice:
```rust
let cldr_slice: &'static [u8] = map_startup_vmo(PA_LOCALE_DATA_VMO)?;
let provider = ZeroCopyDataProvider::new(cldr_slice)?;
let formatter = DateTimeFormatter::try_new_unstable(&provider, &locale, options)?;

```


This model guarantees zero heap copies, instantaneous startup, and complete address-space physical page sharing across all running apps.

---

### 3.4 User Preferences & Dynamic Fallback

BexOS supports independent selection of display language and regional formatting rules, stored in `prefsd` as a structured preference object:

```rust
pub struct UserLocalePreferences {
    /// Ordered fallback chain for UI translations
    pub language_priority: Vec<String>, // e.g. ["es-MX", "es", "en-US"]
    /// Regional formatting preferences
    pub measurement_system: MeasurementSystem, // Metric / US
    pub first_day_of_week: Weekday,             // e.g. Monday
    pub override_region: Option<String>,       // e.g. "de-DE" for currency/dates
    pub hour_cycle: HourCycle,                 // H12 / H23 / H24
}

```

#### String Resolution Algorithm

When an application invokes `t!("network-timeout-detail", count = retry_count)`:

1. The resolver queries the local catalog using the primary language code (`es-MX`).
2. If the message key or specific plural permutation is missing, the resolver falls back to the parent language category (`es`).
3. If still unresolved, it falls back to the declared system fallback baseline (`en-US`).
4. Number interpolation within the message (e.g., formatting `retry_count`) bypasses the string catalog and delegates to the local `ICU4X` instance configured with the active `override_region`.

---

### 3.5 Reactive In-Place Updates (Zero Restart)

When a user selects a new language in `sysui`:

1. `prefsd` commits the updated language hierarchy to `/data/users/<uid>/prefs/locale.bexpref`.
2. `prefsd` notifies `localed`, which broadcasts an event across the active desktop session.
3. In Dioxus Native applications, the top-level `LocaleContext` signal updates:
```rust
#[component]
pub fn App() -> Element {
    use_context_provider(|| Signal::new(LocaleContext::load_current()));
    // ...
}

```


4. Because the `t!(...)` macro consumes the reactive `LocaleContext` signal, any change invalidates all visible text nodes in the component tree.
5. Dioxus computes a minimal DOM/RSX delta.
6. The host runner translates these changes into layout adjustments via Taffy and updates text runs in `cosmic-text`.
7. The interface updates atomically in the new language with zero lost state and zero process restarts.

---

### 3.6 Supplementary Language Pack Delivery via `pkgd`

To preserve flash storage footprints on edge hardware and cloud hypervisor appliances, base operating system images only bundle standard global fallback locales (e.g., `en-US`, minimal CJK baseline).

#### OCI Artifact Specification (`bexos.locale.<lang>`)

Extended locale tables, large regional collation maps, and localized hyphenation dictionaries are packaged as OCI artifacts:

```json
{
  "schemaVersion": 2,
  "mediaType": "application/vnd.oci.image.manifest.v1+json",
  "artifactType": "application/vnd.bexos.locale.pack.v1",
  "config": {
    "mediaType": "application/vnd.bexos.locale.config.v1+json",
    "digest": "sha256:4a2f8c..."
  },
  "layers": [
    {
      "mediaType": "application/vnd.bexos.locale.data.v1",
      "digest": "sha256:7c9e1b...",
      "size": 1845200,
      "annotations": {
        "bexos.org/locale-tag": "ja-JP",
        "bexos.org/cldr-version": "45.0"
      }
    }
  ]
}

```

#### On-Demand Ingestion Workflow

1. When a user selects a language not present in the local `cldr.bexloc` baseline (or an app requests an uncached locale pack), `localed` queries `pkgd`.
2. `pkgd` fetches the package blob from the configured registry, authenticating signatures via the local TUF verification engine.
3. `pkgd` deposits the blob in the local cache partition (`/data/cache/locales/ja-JP.bexloc`).
4. `localed` dynamically constructs a secondary overlay VMO containing the supplementary lookup tables and notifies active runtimes via FIDL to link the extended data provider.

---

## 4. Interface Definition Language (FIDL)

The protocol definitions reside in `idl/bexos/locale/localed.fidl`:

```fidl
library bexos.locale;

using bexos.kernel;

type MeasurementSystem : uint8 {
    METRIC = 1;
    US_CUSTOMARY = 2;
    UK_HYBRID = 3;
};

type HourCycle : uint8 {
    H11 = 1;
    H12 = 2;
    H23 = 3;
    H24 = 4;
};

struct LocaleSettings {
    language_chain vector<string:32>:8; // e.g. ["es-MX", "es", "en-US"]
    region string:32;                   // e.g. "es-MX" or "de-DE"
    measurement MeasurementSystem;
    hour_cycle HourCycle;
    first_day_of_week uint8;            // 1 = Monday, 7 = Sunday
};

@discoverable
protocol LocaleProvider {
    /// Retrieve current active locale settings
    GetLocaleSettings() -> (struct {
        settings LocaleSettings;
    });

    /// Retrieve read-only VMO handle to the active CLDR binary tables
    GetCldrDataVmo() -> (resource struct {
        data zx.Handle:VMO;
        data_len uint64;
    }) error bexos.kernel.Status;
};

@discoverable
protocol LocaleUpdateListener {
    /// Pushed to listening applications when user alters language or format preferences
    OnLocaleSettingsChanged(struct {
        new_settings LocaleSettings;
    });

    /// Pushed when supplementary locale tables are installed via pkgd
    OnCldrDataReloaded(resource struct {
        new_cldr_vmo zx.Handle:VMO;
        data_len uint64;
    });
};

```

---

## 5. Security & Sandboxing Considerations

1. **Zero Filesystem Discovery:** Applications cannot enumerate locale files on disk. Formatting data is strictly delivered through read-only memory handles (`zx.Handle:VMO`), completely closing directory traversal exploit vectors.
2. **Immutable Shared Memory:** The `cldr.bexloc` VMO is mapped strictly with `ZX_VM_PERM_READ`. Any attempt by an exploited sandboxed application to manipulate shared formatting data triggers an immediate memory access violation, isolating the fault to that process.
3. **Memory Safety of Deserialization:** The use of `ICU4X` ensures that zero-copy deserialization of binary data tables operates entirely without raw unsafe pointer offsets, avoiding buffer overruns from corrupted metadata.
4. **Verified Package Ingestion:** Supplementary language assets are fetched solely via `pkgd` under TUF verification. `localed` does not process unsigned, arbitrary language packages directly from the network.

---

## 6. Implementation Roadmap

### Phase 1: Build-Time Tooling & ICU4X Integration

* Implement the Bazel rule (`bexos_locale_bundle`) compiling `.ftl` message trees into zero-copy binary catalogs (`strings.bexres`).
* Generate the base platform `cldr.bexloc` binary using the upstream Unicode CLDR toolchain and ICU4X serialization.
* Implement `lib/userspace/i18n` wrapping the ICU4X zero-copy data provider against raw byte slices.

### Phase 2: Core Service & Process Propagation

* Implement the `localed` service in Rust, loading `/system/data/locale/cldr.bexloc` and binding the `LocaleProvider` FIDL interface.
* Update `appd` to pass the `PA_LOCALE_DATA_VMO` startup handle and user preference chain to newly initialized applications.
* Build the reactive Dioxus `I18nProvider` context to support dynamic, in-place language switching.

### Phase 3: Dynamic OCI Integration

* Implement the OCI artifact schema for `bexos.locale.<lang>`.
* Connect `localed` to `pkgd` to support on-demand background fetching of supplementary locale packs.
* Wire download confirmations and language installation progress into `sysui` settings panels.