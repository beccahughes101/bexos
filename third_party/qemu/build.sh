#!/usr/bin/env bash
set -euo pipefail
source_dir=$(cd "$(dirname "$1")" && pwd)
output=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
layout=$(cd "$(dirname "$3")" && pwd)/$(basename "$3")
extension=$(cd "$(dirname "$4")" && pwd)/$(basename "$4")
compiler=$(cd "$(dirname "$5")" && pwd)/$(basename "$5")
setuptools=$(cd "$(dirname "$6")" && pwd)/$(basename "$6")
ninja=$(cd "$(dirname "$7")" && pwd)/$(basename "$7")
cxx=$(cd "$(dirname "$8")" && pwd)/$(basename "$8")
work=$(mktemp -d "${TMPDIR:-/tmp}/bexos-qemu.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
cp -RL "$source_dir" "$work/source"
cp "$setuptools" "$work/source/python/wheels/"
cp "$layout" "$work/source/hw/arm/bexos_secure_layout.h"
cp "$extension" "$work/source/hw/arm/bexos_secure_memory.c.inc"
python3 -B - "$work/source/hw/arm/virt.c" <<'PY'
from pathlib import Path
import sys
p = Path(sys.argv[1])
s = p.read_text()
anchor = 'static void *machvirt_dtb('
call = '        create_secure_ram(vms, secure_sysmem, secure_tag_sysmem);'
if s.count(anchor) != 1 or s.count(call) != 1:
    raise SystemExit('pinned QEMU secure memory integration point changed')
s = s.replace(anchor, '#include "bexos_secure_memory.c.inc"\n\n' + anchor)
s = s.replace(call, call + '\n        bexos_secure_transition_ram(vms, secure_sysmem);')
p.write_text(s)
PY
# Host development libraries mirror the existing EFI signer dependency model.
# Configure is offline; every QEMU source byte comes from the pinned repository.
host_flags=()
if [[ $(uname -s) == Darwin ]]; then
    export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/glib/lib/pkgconfig:/opt/homebrew/opt/pixman/lib/pkgconfig:/opt/homebrew/opt/libslirp/lib/pkgconfig:/opt/homebrew/opt/dtc/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
    export SDKROOT=$(xcrun --show-sdk-path)
    host_flags+=(--extra-cflags=-I/opt/homebrew/include --extra-ldflags=-L/opt/homebrew/lib)
fi
mkdir "$work/build"
cd "$work/build"
"$work/source/configure" "${host_flags[@]}" --target-list=aarch64-softmmu --cc="$compiler" --cxx="$cxx" --ninja="$ninja" \
    --disable-download --disable-docs --disable-guest-agent --disable-tools \
    --disable-user --disable-werror --enable-slirp --enable-fdt=system \
    --disable-cocoa --disable-gtk --disable-sdl --disable-hvf --disable-kvm
"$ninja" -j 8 qemu-system-aarch64
cp qemu-system-aarch64 "$output"
