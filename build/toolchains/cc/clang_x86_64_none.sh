#!/bin/sh
script_dir=$(CDPATH= cd -- "${0%/*}" && pwd)
exec "$script_dir/clang_common.sh" x86_64-unknown-none-elf -mno-red-zone -mno-sse -mno-sse2 -mno-mmx "$@"
