#!/usr/bin/env bash
set -euo pipefail
cd "${GITHUB_WORKSPACE:?run on a CI runner}"
mkdir -p "$RUNNER_TEMP/bexos-ci-logs"
refresh() {
  local name=$1
  shift
  bazel run -c opt "$@" 2>&1 | tee "$RUNNER_TEMP/bexos-ci-logs/firmware-$name.log"
}

refresh arm --config=aarch64 //third_party/trusty:refresh_image
refresh arm-acceptance --config=aarch64 //third_party/trusty:refresh_authmgr_acceptance_image
refresh x86 --config=x86_64 //third_party/trusty:refresh_x86_64_image
refresh x86-acceptance --config=x86_64 //third_party/trusty:refresh_x86_64_acceptance_image
refresh efi --config=x86_64 //boot/efi:refresh_firmware
refresh efi-acceptance --config=x86_64 --//build/platforms:trusty_variant=acceptance //boot/efi:refresh_firmware

COPYFILE_DISABLE=1 tar -cf "$RUNNER_TEMP/bexos-firmware.tar" \
  third_party/trusty/image.bin \
  third_party/trusty/authmgr_acceptance_image.bin \
  third_party/trusty/x86_64_image.bin \
  third_party/trusty/x86_64_acceptance_image.bin \
  boot/efi/standard_firmware.bin \
  boot/efi/acceptance_firmware.bin
