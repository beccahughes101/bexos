#!/bin/sh
exec "${0%/*}/clang_common.sh" x86_64-unknown-linux-gnu x86_64 "$@"
