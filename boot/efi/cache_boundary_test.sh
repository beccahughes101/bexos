#!/usr/bin/env bash
set -euo pipefail
# RFC 64 requires the protected package-state endpoint in the signed product.
# Keep the dependency boundary test, now proving that source firmware reaches it.
if ! grep -Fxq '//third_party/trusty:trusty_x86_64_firmware' "$1"; then
    echo 'The x86 product omits the source-built Trusty package-state endpoint.' >&2
    exit 1
fi
