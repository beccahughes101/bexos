#!/usr/bin/env bash
set -euo pipefail
# This script changes only disposable GitHub-hosted runners, never workstations.
[[ ${GITHUB_ACTIONS:-} == true && ${RUNNER_ENVIRONMENT:-} == github-hosted && $(uname -s) == Linux ]]
mkdir -p "$RUNNER_TEMP/bexos-ci-logs"
df -h "$GITHUB_WORKSPACE" "$RUNNER_TEMP"

# Reclaim unused SDK space before fetching the large Trusty and Rust closures.
sudo rm -rf /usr/share/dotnet /usr/local/lib/android /opt/ghc /usr/local/.ghcup
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  build-essential bison flex device-tree-compiler xxd libclang-dev llvm-dev \
  libssl-dev pkg-config python3 python3-cryptography python3-pyelftools \
  git curl ca-certificates unzip zip zstd qemu-system-arm qemu-system-x86 qemu-utils
# Use the distro Python together with its installed modules in repository rules,
# build actions, and tests, independently of the runner's hosted toolcache.
echo /usr/bin >> "$GITHUB_PATH"
df -h "$GITHUB_WORKSPACE" "$RUNNER_TEMP"
