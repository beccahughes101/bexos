#!/usr/bin/env bash
set -euo pipefail

protoc=""
manifest_proto=""
well_known_protos=""

while (($# > 0)); do
  case "$1" in
    --protoc)
      protoc="$2"
      shift 2
      ;;
    --manifest-proto)
      shift
      while (($# > 0)) && [[ "$1" != --* ]]; do
        case "$1" in
          */idl/bexos/app/manifest.proto|idl/bexos/app/manifest.proto)
            manifest_proto="$1"
            ;;
        esac
        shift
      done
      ;;
    --well-known-protos)
      shift
      while (($# > 0)) && [[ "$1" != "--" ]]; do
        well_known_protos+="${well_known_protos:+ }$1"
        shift
      done
      ;;
    --)
      shift
      break
      ;;
    *)
      echo "unknown option: $1" >&2
      exit 1
      ;;
  esac
done

if [[ -z "$protoc" || -z "$manifest_proto" || -z "$well_known_protos" ]]; then
  echo "missing protoc, manifest proto, or well-known proto arguments" >&2
  exit 1
fi

workspace_root="${manifest_proto%/idl/bexos/app/manifest.proto}"
if [[ -z "$workspace_root" || "$workspace_root" == "$manifest_proto" ]]; then
  workspace_root="."
fi
protobuf_root=""
for proto in $well_known_protos; do
  case "$proto" in
    */google/protobuf/any.proto)
      protobuf_root="${proto%/google/protobuf/any.proto}"
      ;;
  esac
done
if [[ -z "$protobuf_root" ]]; then
  echo "could not find google/protobuf/any.proto in well-known proto runfiles" >&2
  exit 1
fi
current_manifest=""
decoded="$(mktemp "${TEST_TMPDIR:-/tmp}/d1-manifest.XXXXXX")"

decode_manifest() {
  "$protoc" \
    --proto_path="$workspace_root" \
    --proto_path="$protobuf_root" \
    --decode=bexos.app.Manifest \
    idl/bexos/app/manifest.proto \
    < "$1" \
    > "$decoded"
}

expect_service() {
  local service_name="$1"
  local protocol="$2"
  local lifecycle="$3"
  local ordinals="$4"
  local block

  block="$(
    awk -v expected="$service_name" '
      $1 == "services_exposed" && $2 == "{" {
        in_block = 1
        depth = 1
        block = $0 "\n"
        next
      }
      in_block {
        block = block $0 "\n"
        if ($0 ~ /{/) depth++
        if ($0 ~ /}/) depth--
        if (depth == 0) {
          if (block ~ "name: \"" expected "\"") {
            printf "%s", block
            found = 1
            exit
          }
          in_block = 0
        }
      }
      END { if (!found) exit 1 }
    ' "$decoded"
  )" || {
    echo "missing service $service_name in $current_manifest" >&2
    exit 1
  }

  grep -q "protocol: \"$protocol\"" <<< "$block" || {
    echo "$service_name has wrong or missing protocol $protocol" >&2
    exit 1
  }
  grep -q "lifecycle: $lifecycle" <<< "$block" || {
    echo "$service_name has wrong or missing lifecycle $lifecycle" >&2
    exit 1
  }

  IFS="," read -ra expected_ordinals <<< "$ordinals"
  for ordinal in "${expected_ordinals[@]}"; do
    grep -q "method_ordinals: $ordinal" <<< "$block" || {
      echo "$service_name missing method ordinal $ordinal" >&2
      exit 1
    }
  done
}

for arg in "$@"; do
  if [[ "$arg" != *"|"* ]]; then
    current_manifest="$arg"
    decode_manifest "$current_manifest"
    continue
  fi

  if [[ -z "$current_manifest" ]]; then
    echo "service expectation appeared before a manifest path" >&2
    exit 1
  fi

  IFS="|" read -r service_name protocol lifecycle ordinals <<< "$arg"
  expect_service "$service_name" "$protocol" "$lifecycle" "$ordinals"
done
