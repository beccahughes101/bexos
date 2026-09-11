# RFC 0018: Static asset packages

- Created: 2026-08-27T22:28:39-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Code-less asset bundles use ordinary package dependencies and private /deps mounts. Resource groups let consumers request only the assets they need.

## Design overview

Static asset bundles, such as root CA certificates, timezone databases, ML models, and UI icon packs, are **code-less data packages**. Consumers declare them as dependencies and access them through `/deps/<package_name>` in their capability-scoped process namespace.

## Manifest Declaration (Dependency Specification)

A code-less asset package and a consuming service define their relationship in their manifests:

### The Asset Package Manifest (`//packages/ca_certificates/manifest.prototxt`)

```protobuf
package_name: "bexos.data:root_certificates"
package_version { major: 2026 minor: 8 patch: 1 }
multi_version_policy: SINGLE_ACTIVE_ONLY

// Pure asset package: no executables/processes declared
package_type: ASSET_BUNDLE

assets {
  path: "certs/cacert.pem"
  content_type: "application/x-pem-file"
}

```

## Resource Groups

Packages can declare flat CPU-share resource groups in their prototxt manifest
and assign individual processes by name:

```protobuf
package_name: "com.example:camera"
name: "Camera"

resource_groups {
  name: "camera_foreground"
  cpu_shares: 768
  memory_limit_pages: 131072
}

processes {
  name: "camera_service"
  runner: "elf"
  resource_group: "camera_foreground"
  runner_options {
    [type.googleapis.com/bexos.app.ELFRunnerOptions] {
      path: "/pkg/bin/camera_service"
    }
  }
}
```

`resource_group` may also name one of the built-ins: `system`, `foreground`,
`background`, or `driver`. If a process omits the field, appd chooses the
default group from package identity and driver policy: platform core services use
`system`, direct hardware drivers use `driver`, and regular apps use
`foreground`.

### Consuming Service Manifest (`//services/netstack/manifest.prototxt`)

```protobuf
package_name: "bexos.service:netstack"
processes {
  name: "netstack_daemon"
  runner: "wasm"
}

library_dependencies {
  package_name: "bexos.data:root_certificates"
  version_requirement: "v2026.8"
  mount_alias: "root_certs"
}
library_dependencies {
  package_name: "bexos.data:iana_tzdb:v2026.1"
}

```

## Namespace Layout for Consuming Processes

When `appd` launches the `netstack` process, it builds its private virtual namespace table:

```
[ Process Namespace: bexos.service:netstack ]
├── /pkg                               <-- Its own binary & manifest
│   ├── bin/netstack.wasm
│   └── manifest.pb
│
├── /deps                              <-- Mounted read-only dependency packages
│   ├── root_certs/                    <-- Mounted from bexos.data:root_certificates.bex
│   │   └── certs/cacert.pem
│   └── bexos.data:iana_tzdb/          <-- Mounted from iana_tzdb.bex
│       └── zoneinfo/
│
├── /shared/<vault_name>               <-- Shared cross-app mutable data
└── /data                              <-- Private encrypted state

```

## Dependency namespace properties

* **Zero Duplication in RAM/Storage:** The `.bex` asset archive for `root_certificates` exists once on disk. When 50 different apps depend on `bexos.data:root_certificates`, `appd` maps the exact same read-only `ArchiveFS` directory handle into every app's `/deps/root_certs` namespace.

* **Independent Updates:** An urgent root certificate revocation or timezone update can be delivered over the network as a standalone `.bex` asset package without recompiling or redeploying the `netstack` or browser binaries.
* **Hermetic Path Resolution:** The application opens `/deps/root_certs/certs/cacert.pem`. The POSIX/WASI path resolver in libc maps `/deps` directly to the capability handle injected by `appd`.

* **Sandboxed by Default:** A malicious app cannot traverse outside its declared dependencies (`/deps/some_unauthorized_package` does not exist in its namespace).
