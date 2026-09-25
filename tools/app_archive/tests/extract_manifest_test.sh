#!/usr/bin/env bash
set -euo pipefail

tool="$1"
wasm="$2"
native="$3"
public="$4"
wrong_public="$5"
wrong_architecture="$6"
tmp="${TEST_TMPDIR:-/tmp}"

"$tool" extract-manifest --archive "$wasm" --public-key "$public" \
  --package-id bexos.sdk.fixture.wasm --architecture AARCH64 \
  --maximum-abi-version 1 --out "$tmp/manifest"

if "$tool" extract-manifest --archive "$wasm" --public-key "$wrong_public" \
  --package-id bexos.sdk.fixture.wasm --architecture AARCH64 \
  --maximum-abi-version 1 --out "$tmp/wrong-signer"; then
  echo "wrong signer accepted" >&2
  exit 1
fi

if "$tool" extract-manifest --archive "$wasm" --public-key "$public" \
  --package-id bexos.sdk.fixture.other --architecture AARCH64 \
  --maximum-abi-version 1 --out "$tmp/wrong-id"; then
  echo "wrong package id accepted" >&2
  exit 1
fi

if "$tool" extract-manifest --archive "$wasm" --public-key "$public" \
  --package-id bexos.sdk.fixture.wasm --architecture AARCH64 \
  --maximum-abi-version 0 --out "$tmp/future-abi"; then
  echo "future ABI accepted" >&2
  exit 1
fi

cp "$wasm" "$tmp/tampered.bex"
chmod u+w "$tmp/tampered.bex"
printf '\001' | dd of="$tmp/tampered.bex" bs=1 seek=40 conv=notrunc status=none
if "$tool" extract-manifest --archive "$tmp/tampered.bex" --public-key "$public" \
  --package-id bexos.sdk.fixture.wasm --architecture AARCH64 \
  --maximum-abi-version 1 --out "$tmp/tampered"; then
  echo "tampered archive accepted" >&2
  exit 1
fi

printf 'unsigned' > "$tmp/unsigned.bex"
if "$tool" extract-manifest --archive "$tmp/unsigned.bex" --public-key "$public" \
  --package-id bexos.sdk.fixture.wasm --architecture AARCH64 \
  --maximum-abi-version 1 --out "$tmp/unsigned"; then
  echo "unsigned archive accepted" >&2
  exit 1
fi

if "$tool" extract-manifest --archive "$native" --public-key "$public" \
  --package-id bexos.sdk.fixture.native --architecture "$wrong_architecture" \
  --maximum-abi-version 1 --out "$tmp/wrong-architecture"; then
  echo "wrong architecture accepted" >&2
  exit 1
fi
