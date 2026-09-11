#!/usr/bin/env bash
set -euo pipefail
validator=$1
read -r -a workflows <<< "$2"
# actionlint 1.7.12 predates GitHub's ubuntu-26.04 public-preview runner.
# Its custom-label configuration extends validation without suppressing other
# runner-label mistakes. This is a tool input, not a self-hosted runner choice.
config=$(mktemp "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/actionlint.XXXXXX")
trap 'rm -f "$config"' EXIT
printf 'self-hosted-runner:\n  labels: [ubuntu-26.04]\n' > "$config"
"$validator" -config-file "$config" -shellcheck= -pyflakes= "${workflows[@]}"
[[ $3 == --scripts ]]
shift 3
for script in "$@"; do bash -n "$script"; done
