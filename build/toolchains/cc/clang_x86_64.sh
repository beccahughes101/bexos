#!/bin/sh
script_dir=$(CDPATH= cd -- "${0%/*}" && pwd)
exec "$script_dir/clang_common.sh" x86_64-unknown-linux-gnu "$@"
