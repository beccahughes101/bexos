#!/usr/bin/env bash
set -euo pipefail
protoc="$1"
product="$2"
arch="$3"
definition="$4"
policy="$5"
index="$6"
protobuf_root=""
for proto in $7; do
  case "$proto" in */google/protobuf/any.proto) protobuf_root="${proto%/google/protobuf/any.proto}";; esac
done
[[ -n "$protobuf_root" ]]

decoded="$("$protoc" --proto_path=. --proto_path="$protobuf_root" --decode=bexos.platform.ProductDefinition idl/bexos/platform/assembly.proto < "$definition")"
[[ "$decoded" == *"product_name: \"${product}_${arch}\""* ]]
[[ "$decoded" == *"board: \"//device/virtual/qemu/base/${arch}\""* ]]
[[ "$decoded" == *"platform_config: \"//device/virtual/qemu/${product}:platform_config_bin\""* ]]
[[ "$decoded" == *'bundles: "base"'* && "$decoded" == *'bundles: "qemu_hardware"'* ]]
if [[ "$product" == workstation ]]; then
  [[ "$decoded" == *'bundles: "graphics"'* && "$decoded" == *'bundles: "qemu_graphics"'* ]]
else
  [[ "$decoded" != *'bundles: "graphics"'* && "$decoded" != *'bundles: "qemu_graphics"'* ]]
fi
assembled="$(cat "$index")"
[[ "$assembled" == *"product_name: ${product}_${arch}"* ]]
[[ "$assembled" == *'package: bexos.service.teed '* ]]
[[ "$assembled" == *'package: bexos.lib.tee_driver.trusty '* ]]
if [[ "$product" == workstation ]]; then
  for package in bexos.driver.display.virtio_gpu bexos.service.splashd bexos.service.fontd bexos.service.scened; do
    [[ "$assembled" == *"package: $package"* ]]
  done
else
  [[ "$assembled" != *"package: bexos.service.splashd"* && "$assembled" != *"package: bexos.service.fontd"* && "$assembled" != *"package: bexos.service.scened"* && "$assembled" != *"package: bexos.driver.display.virtio_gpu"* ]]
fi
policy_text="$("$protoc" --proto_path=. --proto_path="$protobuf_root" --decode=bexos.platform.PlatformConfig idl/bexos/platform/config.proto < "$policy")"
if [[ "$arch" == aarch64 ]]; then
  [[ "$policy_text" == *'enforce_secure_boot: true'* && "$policy_text" == *'rpmb_anti_rollback: true'* ]]
  for app in keymint gatekeeper storage avb authmgr; do
    [[ "$policy_text" == *"allowed_trusted_apps: \"bexos.ta.${app}\""* ]]
  done
  [[ "$policy_text" == *'allowed_trusted_apps: "bexos.orchestrator"'* ]]
else
  [[ "$policy_text" == *'enforce_secure_boot: true'* && "$policy_text" == *'rpmb_anti_rollback: true'* ]]
fi
[[ "$policy_text" != *'allowed_trusted_apps: "bexos.ta.confirmation'* ]]
echo "${product}/${arch}: product identity, graphics bundles, Trusty packages and policy verified"
