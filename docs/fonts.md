# Local fonts

BexOS implements the local portion of [RFC 0063](rfcs/0063/README.md) through
the wave-5, heart-transplant-capable `fontd` service. Its generated
`bexos.fonts.FontProvider` FIDL contract has three explicit ordinals:
`ResolveFont`, `GetFallbackList`, and permission-gated `InstallUserFont`.

The immutable system tier contains Inter Variable, JetBrains Mono Variable,
Noto Sans Variable, Noto Sans Arabic Variable, and Noto Sans Devanagari Variable.
Bazel pins the exact artifacts and licenses, supplies the same labels to
`fontd`'s static index, and writes the files and OFL texts into every QEMU system
image under `/system/data/fonts`.

The user tier is the `fonts` directory in the encrypted user home. `fontd`
checks the authenticated caller through usersd, opens the home through vfsd only
while it is unlocked, loads it lazily, and evicts it on lock or deletion. Installs
are digest-named and idempotent. The install method never accepts a UID.

Before persistence or exposure, `fontd` validates SFNT/TTC sizes, face and table
bounds, required tables, duplicate tags and partial overlaps, names, style,
weight, and script coverage. TTF, OTF/CFF, variable fonts, and TTC collections
are supported. WOFF2 is reserved in the ABI and rejected in the current release.

Canonical VMOs are keyed by SHA-256. Returned handles have only transfer, read,
and map rights. `//lib/font_client` verifies those rights, maps each VMO read-only,
caches it by face identity, and exposes Arc-backed bytes to `flatland_text`
without copying. Scened and the native Dioxus host use this path for Parley
shaping; consumer caches are deliberately reconstructed after heart transplant.

Configured remote misses now queue through `lib/pkg_client` when
`allow_network_fetch=true`. Local fonts retain priority. Fontd validates the
returned metadata and forwards pkgd's immutable VMO; known remote digests can
use the resolver cache. Product defaults contain no remote mappings or trust
roots. This path has not yet passed guest OCI acceptance; see
[RFC 0064 current state](rfcs/0064/CURRENT.md). Fontd adds no network capability
and does not create a separate `/data/cache/fonts` store.

Remote font indexing, immutable VMO reuse, isolation and continuity across pkgd
replacement still need guest acceptance. Passing local-font and parser tests
does not validate those paths. Resolver transport and lifecycle test gaps are
listed in [RFC 0064's gap table](rfcs/0064/CURRENT.md#current-gaps).
