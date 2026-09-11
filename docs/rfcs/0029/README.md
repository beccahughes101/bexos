# RFC 0029: Origin-based packages and domain verification

- Created: 2026-08-30T10:27:04-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Origin-prefixed package identities and verified domain associations govern installation and link handling. The design covers well-known manifests, cached policy records, and alternative domain-verification mechanisms.

## Design overview

This design establishes origin-based package namespaces, domain verification via well-known manifests, and live domain association updates in `appd`.

Implementation note: the current repository has the `DomainPolicyRecord`
protobuf schema, a manual codec plus redb-backed `lib/domain_association`
cache, the `bexos.app.manager.AppManager` FIDL surface, appd-owned in-memory and
`domain_associations.redb` persistence for reload results, and debugd/bexctl
routing for `install-url` and `reload-well-known`. The production guest HTTPS
fetcher and full direct-from-web archive install pipeline remain future work
under this design.

## Origin-Prefixed Package Identifier Model

To prevent package impersonation and namespace collisions across the open web, every package downloaded directly from a web domain prefixes its reversed fully qualified domain name (FQDN):

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ Web Origin: https://monzo.com/apps/banking.bex                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ • Canonical Domain: monzo.com                                               │
│ • Reversed Domain Prefix: com.monzo                                         │
│ • Local Package Suffix in Manifest: monzo                                   │
│ • Fully Qualified Package ID: com.monzo:monzo                               │
└─────────────────────────────────────────────────────────────────────────────┘

```

* **User Installed Packages:** Packages from non-web interfaces (e.g. debugd) have colon separator with user prefix (e.g., `user:com.bexos.browser`).

* **Platform/Store Packages:** Packages from signed OS bundles keep standard reverse-DNS identifiers with a "system" preflix (e.g., `system:com.bexos.browser`).

* **Web-Installed Packages:** Direct-from-web apps are strictly normalized as `<reversed-origin-domain>:<package-suffix>` (e.g., `com.monzo:monzo`, `io.github.user:pdf-reader`). Reject install if there is no valid well-known file.

## `appd` Installation & Well-Known Lifecycle

```
[ User triggers InstallAppFromUrl("https://monzo.com/app.bex") ]
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. Download & Verify Package (.bex)                                         │
│    • Fetch package bytes and parse internal `manifest.proto`                │
│    • Extract package declared suffix `monzo` and Ed25519 signing key        │
│    • Form candidate package ID: `com.monzo:monzo`                           │
└─────────────────────────────┬───────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. Fetch & Validate Domain Well-Known Association                           │
│    • GET https://monzo.com/.well-known/bexos-manifest.json                  │
│    • Verify: Candidate package ID is listed in `allowed_packages`           │
│    • Verify: Binary signing key matches `signing_fingerprints`              │
└─────────────────────────────┬───────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. Commit to System & User Stores                                           │
│    • Move .bex to `/system/packages/com.monzo:monzo.bex`                    │
│    • Register verified URL prefixes in `opener.redb`                        │
│    • Record domain trust record in `/system/state/domain_associations.redb` │
└─────────────────────────────────────────────────────────────────────────────┘

```

## `appd` FIDL Protocol

```fidl
library bexos.app;

using bexos.kernel;

type InstallSource = strict union {
    1: store_target string:128;
    2: download_url string:2048;
};

type AssociationStatus = strict enum : uint8 {
    VERIFIED                 = 1;
    DOMAIN_UNREACHABLE       = 2;
    SIGNING_KEY_MISMATCH     = 3;
    ORIGIN_MISMATCH          = 4;
};

@discoverable
protocol AppManager {
    /// Download, namespace, verify against well-known, and stage an app from a web URL
    InstallAppFromUrl(struct {
        url string:2048;
    }) -> (struct {
        status bexos.kernel.Status;
        allocated_package_id string:128; // e.g. "com.monzo:monzo"
        association AssociationStatus;
    });

    /// Re-fetch and update the well-known manifest for an origin domain
    ReloadWellKnownForDomain(struct {
        domain string:255;
    }) -> (struct {
        status bexos.kernel.Status;
        association AssociationStatus;
        updated_handlers_count uint32;
    });

    /// Query verified domain associations for a given package
    GetDomainAssociation(struct {
        package_id string:128;
    }) -> (struct {
        status bexos.kernel.Status;
        domain string:255;
        association AssociationStatus;
        last_verified_timestamp uint64;
    });
};

```

## Security & Invariant Rules

* **Prefix Enforcement:** `appd` never allows an app fetched from `monzo.com` to register an unqualified ID or an ID belonging to a different domain (e.g., `com.paypal:wallet` from `monzo.com` is rejected immediately).
* **Universal Link Validation:** A web-installed app can only intercept `https://monzo.com/*` universal links if `ReloadWellKnownForDomain` verifies the site's `.well-known` file explicitly permits that package's Ed25519 signing key.

The `.well-known/bexos-manifest.json` schema below defines additional authorization rules for `appd` to validate during installation and link resolution.

## Extended Well-Known Manifest (`bexos-manifest.json`)

```json
{
  "origin": "https://waymo.com",
  "allowed_package_prefixes": [
    "com.waymo"
  ],
  "trusted_distribution_origins": [
    "https://cdn.waymo.com",
    "https://distribution.bexos.org"
  ],
  "trusted_peer_domains": [
    "https://google.com",
    "https://alphabet.com"
  ],
  "signing_certificate_fingerprints": [
    "SHA256:4a:6f:8b:..."
  ]
}

```

## How `appd` Validates the Expanded Fields

During `InstallAppFromUrl` and `ReloadWellKnownForDomain`, `appd` runs through a five-stage verification pipeline:

```
[ User triggers InstallAppFromUrl("https://cdn.waymo.com/packages/rider.bex") ]
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. Distribution Origin Check                                                │
│    • Download source `https://cdn.waymo.com` is verified against             │
│      `trusted_distribution_origins` in `waymo.com` well-known manifest.     │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. Package Prefix Authorization                                             │
│    • Manifest declares package ID: `com.waymo:rider`                        │
│    • `appd` verifies `com.waymo` matches an entry in                        │
│      `allowed_package_prefixes`.                                            │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. Certificate Fingerprint Match                                            │
│    • Computes SHA256 of the package signing public key / certificate.       │
│    • Verifies it is listed in `signing_certificate_fingerprints`.            │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 4. Cross-Domain Deep Link Verification (Trusted Peers)                      │
│    • If `com.waymo:rider` registers universal links for `google.com/*`,     │
│      `appd` confirms `google.com` is in `trusted_peer_domains`.             │
│    • `appd` checks reverse association on `google.com/.well-known/...`.     │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Protobuf Definition for Domain Association Records

Stored in `/system/state/domain_associations.redb`:

```protobuf
syntax = "proto3";

package bexos.domain;

message DomainPolicyRecord {
  string origin = 1;
  repeated string allowed_package_prefixes = 2;
  repeated string trusted_distribution_origins = 3;
  repeated string trusted_peer_domains = 4;
  repeated string signing_certificate_fingerprints = 5;
  uint64 fetched_timestamp = 6;
  uint64 ttl_seconds = 7;
}

```

## Validation Invariants Enforced by `appd`

* **Distribution Origin Whitelist:** Prevents arbitrary third-party mirrors or CDN hijacks from staging authorized packages unless explicitly listed in `trusted_distribution_origins`.
* **Sub-Domain / Sub-Namespace Delegation:** `allowed_package_prefixes` enables organizations to publish multiple distinct app identifiers under one root domain policy (e.g., `com.waymo:rider`, `com.waymo:driver`, `com.waymo:fleet`).
* **Bidirectional Peer Trust:** Universal link routing between `trusted_peer_domains` (e.g., Waymo handling a [google.com/maps/ride](https://google.com/maps/ride) link) requires a bidirectional handshake—both domains must list each other as trusted peers in their respective `.well-known` manifests.

* **Association Expiration:** `appd` caches `.well-known` records in `/system/state/domain_associations.redb` with a 7-day TTL, periodically triggering `ReloadWellKnownForDomain` in the background. If a domain drops support or changes signing keys, link intercept authority is automatically revoked.

If a developer is hosted on a restrictive CMS (e.g., Squarespace, Shopify, Medium, or custom site-builders) where uploading custom static files under `/.well-known/` is blocked, several standard cryptographic and platform-level verification alternatives can prove domain ownership and establish trust:

## DNS `TXT` Record Verification (Recommended)

DNS records are completely decoupled from CMS web servers and managed directly at the domain registrar or DNS provider (Cloudflare, Route 53, Namecheap, etc.).

* **Mechanism:** The developer adds a standardized `TXT` record containing either a compact pointer URL or an inline Base64/minified payload.

### DNS Record Entry

```text
_bexos-origin.waymo.com. IN TXT "v=bex1; pkg=com.waymo; dist=https://cdn.waymo.com,https://distribution.bexos.org; fp=SHA256:4a:6f:8b:..."

```


### Pointer Delegation Mode (if payload is large)

```text
_bexos-manifest.waymo.com. IN TXT "manifest_uri=https://raw.githubusercontent.com/waymo/bexos-origin/main/manifest.json"

```


* **Validation Flow:** `cert-tools` or `app_service` executes a DNS-over-HTTPS (DoH) or DNSSEC query against `_bexos-origin.waymo.com` to verify domain association without hitting the web server.

## Verified Mark Certificate (VMC) / BIMI & TLS SAN Extensions

The existing `cert-tools` infrastructure can embed domain-to-package bindings directly into X.509 certificates:

### X.509 Certificate SAN & Custom OID Extensions

* When issuing the app signing certificate, the developer includes their verified domain in the Subject Alternative Name (SAN) or a custom BexOS ASN.1 extension OID.
* An automated ACME / Let's Encrypt-style DNS-01 challenge validates domain ownership during certificate generation.

* **Result:** The app package is self-attesting. The OS verifies the signature chain on the `.bexapp` package against the embedded domain certificate without making runtime HTTP calls.

## HTML `<meta>` Tag Verification

Most CMS platforms allow injecting custom tags into the `<head>` of pages (via Google Tag Manager, custom header scripts, or SEO settings):

* **Mechanism:** The developer adds a metadata tag to their root home page ([https://waymo.com/](https://waymo.com/)):
```html
<meta name="bexos-association-manifest"
      content="data:application/json;base64,eyAib3JpZ2luIjogImh0dHBzOi8vd2F5bW8uY29tIiwgImFsbG93ZWRfcGFja2FnZV9wcmVmaXhlcyI6IFsiY29tLndheW1vIl0sIC4uLn0=">

```


*(Or point to an external CDN/GitHub URL: `<meta name="bexos-manifest-url" content="https://cdn.example.com/bexos-manifest.json">`)*
* **Validation Flow:** The verifier requests `GET https://waymo.com/`, parses HTML `<head>` tags for `name="bexos-association-manifest"`, and decodes the JSON payload.

## Public Code Repository & OIDC Identity Binding (GitHub / GitLab)

For developer-focused deployments, bind the domain using GitHub Pages or Sigstore/Cosign keyless signing:

### Mechanism

* The developer publishes the manifest file at a verified GitHub repository ([https://github.com/waymo/.bexos/blob/main/manifest.json](https://github.com/waymo/.bexos/blob/main/manifest.json)).
* A GitHub Action builds and signs the `.bexapp` using OpenID Connect (OIDC).

* **Validation Flow:** The BexOS Store or verifier inspects the OIDC token claims (attesting repository identity and commit SHA) alongside the repository-hosted manifest.

## Priority Resolution Order for `cert-tools` / Verification Daemon

Host verification tools (`tools/cert-tools/`) probe the supported mechanisms in the following order:

```
1. Well-Known HTTP Endpoint  ──►  https://waymo.com/.well-known/bexos-manifest.json
2. DNS TXT Record Lookup    ──►  _bexos-origin.waymo.com
3. HTML <head> Meta Tag     ──►  GET https://waymo.com/ (parse <meta name="bexos-...">)
4. X.509 Embedded Extension  ──►  Read SubjectAltName / Custom OID from Package Cert

```
