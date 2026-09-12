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

Dynamic OCI/TUF fetching is not active. `allow_network_fetch` is stable ABI for
future work, and a disabled missing-font resolver currently returns `NOT_FOUND`
without contacting pkgd, using the network, writing `/data/cache/fonts`, or
showing a download prompt.
