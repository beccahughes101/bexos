#!/usr/bin/env bash
set -euo pipefail
cd "${TEST_SRCDIR}/${TEST_WORKSPACE}"
exec python3 testing/e2e/qemu/venus_linux/elf_stack_test.py
