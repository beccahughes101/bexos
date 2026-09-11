#!/usr/bin/env bash
set -euo pipefail
[[ ${GITHUB_ACTIONS:-} == true && ${RUNNER_ENVIRONMENT:-} == github-hosted && $(uname -s) == Linux ]]
# Tests normally reap their children. Also handle aborted or failed harnesses.
# These runners are disposable and dedicated to this job; leave Bazel alone.
pattern='(^|/)(qemu-system-(aarch64|x86_64)|rpmb_dev)( |$)'
if pgrep -u "$(id -u)" -f "$pattern"; then
  pkill -TERM -u "$(id -u)" -f "$pattern" || [[ $? == 1 ]]
  for _ in {1..10}; do
    if ! pgrep -u "$(id -u)" -f "$pattern"; then exit 0; fi
    sleep 1
  done
  pkill -KILL -u "$(id -u)" -f "$pattern" || [[ $? == 1 ]]
  sleep 1
  if pgrep -u "$(id -u)" -f "$pattern"; then
    echo 'Guest processes survived cleanup' >&2
    exit 1
  fi
fi
