# RFC-0063 current implementation

The local font architecture is implemented. The QEMU system images contain the
Bazel-pinned Inter Variable, JetBrains Mono Variable, Noto Sans Variable, Noto
Sans Arabic Variable, and Noto Sans Devanagari Variable files under
`/system/data/fonts`, with all five OFL license files. The workstation BootFS
also contains the wave-5 `bexos.service.fontd` manifest and ELF; its replacement
archive is assembled by Bazel.

`fontd` exposes `bexos.fonts.FontProvider` with explicit ordinals for resolve,
fallback, and install. Resolve and fallback are public. Install is a separate
`bexos.permission.INSTALL_USER_FONTS` capability and uses only the authenticated
UID in appd's service-binding metadata. Callers cannot choose a target UID.

The service validates bounded SFNT and TTC containers, table bounds, duplicate
tags, partial overlaps, required metadata tables, family/style/weight metadata,
collection face counts, and representative Latin/Arabic/Devanagari coverage.
It accepts TrueType, OpenType/CFF, variable SFNT files, and TTC collections.
WOFF2 is represented by the ABI but returns `UNSUPPORTED_FORMAT` in this release.
Full SHA-256 digests deduplicate canonical VMOs, and per-face IDs are derived
stably from the digest and collection index.

Matching normalizes family whitespace and case, selects the caller's user fonts
before system fonts, applies exact/normal style preference, nearest CSS weight,
format preference, and a stable ID tie-break. Fallback lists are bounded to eight
entries for `Latn`, `Arab`, and `Deva`; caller-owned fonts precede the configured
Inter/Noto system baseline. Unsupported script tags return `INVALID_ARGS`.

User fonts live in the unlocked user's encrypted home `fonts` directory, which
corresponds to `/data/users/<uid>/fonts`. Loading is lazy and gated by usersd.
Digest-named installs are idempotent; failed writes are unlinked. Lock and delete
events evict that UID's index entries and orphaned canonical VMOs. User scope is
applied to every lookup.

Canonical service VMOs retain `TRANSFER | READ | MAP | DUPLICATE`; clients receive
only `TRANSFER | READ | MAP`. The shared native client verifies those rights,
maps the VMO read-only, caches by `(font_id, collection_index)`, and supplies an
Arc-backed Parley blob whose final drop unmaps and closes the handle. `fontd`
migrates its index, clients, user watcher, storage/service endpoints, and active
VMOs during heart transplant.

Scened resolves its Inter and Noto shaping inputs from `fontd` on BexOS instead
of embedding guest font copies. The WASM/Dioxus native host resolves CSS family
stacks through the same client cache, uses Inter and JetBrains Mono baselines,
selects Arabic and Devanagari fallbacks, emits Parley glyph IDs, and gives Vello
the mapped font data for glyph rendering. Retained documents migrate; mapped
font and shaping caches are re-resolved after adoption. Signed package-local
font assets remain supported by the existing scene asset path.

Configured remote misses now use pkgd through `lib/pkg_client`, with local
matching taking priority and the existing parser validating returned metadata.
Only misses with `allow_network_fetch=true` may trigger named network resolution;
known digests otherwise use cache-only calls. Default products have no remote
font mappings. Guest OCI acceptance, WOFF2 decoding, SysUI download prompts and
Servo-specific consumers remain outstanding. CAS storage belongs to pkgd;
fontd does not gain a network capability or a separate `/data/cache/fonts`.


RFC 0064 now adds the centralized pkgd source implementation, including a
configured font miss path, asynchronous app installation, OCI/TUF verification
and a protected package-state endpoint. Default products contain no remote
registry roots or mappings. The complete guest/lifecycle acceptance remains
outstanding; see [package resolution current state](../0064/CURRENT.md).
In particular, remote font indexing, immutable VMO reuse, user isolation and
continuity through pkgd replacement have not passed guest acceptance. Existing
local-font results do not establish remote resolution; see the resolver's
[implementation and validation gaps](../0064/CURRENT.md#current-gaps).
