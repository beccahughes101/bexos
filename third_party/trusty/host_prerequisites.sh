#!/bin/bash
# Sourced by the firmware recipe; keep dependency failures before upstream make.
make_cmd=make
if command -v gmake >/dev/null 2>&1; then make_cmd=gmake; fi
for tool in "$make_cmd" python3 dtc xxd; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "Trusty requires $tool on the execution host" >&2
    return 1
  }
done
if command -v gsed >/dev/null 2>&1; then
  sed_cmd="$(command -v gsed)"
elif sed --version >/dev/null 2>&1; then
  sed_cmd="$(command -v sed)"
else
  echo "Trusty requires GNU sed (gsed on Darwin)" >&2
  return 1
fi
