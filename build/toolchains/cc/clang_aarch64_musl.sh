#!/bin/sh
exec "${0%/*}/clang_common.sh" aarch64-unknown-linux-musl "$@"
