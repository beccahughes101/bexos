#!/bin/sh
set -eu
while IFS= read -r target; do
  case "$target" in
    *//lib/net:*|*//services/pkgd:*|*//services/netstack:*|*rustls*|*h2-*|*//:h2|*//:reqwest)
      echo "Forbidden package-client dependency: $target" >&2
      exit 1
      ;;
  esac
done < "$1"
