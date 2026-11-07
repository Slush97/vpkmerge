#!/usr/bin/env bash
# Fail if a Linux binary references a glibc symbol version above the floor.
# Grimoire ships the release CLI to Ubuntu 22.04 and newer; a binary linked
# against a newer glibc dies at load with "version `GLIBC_x.y' not found".
#
# Usage: tools/check-glibc-floor.sh <binary> <floor>   e.g. ... vpkmerge 2.35
set -euo pipefail

bin=$1
floor=$2

max=$(objdump -T "$bin" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sed 's/^GLIBC_//' | sort -Vu | tail -n1)
echo "$bin: highest glibc symbol version $max (floor $floor)"

if [ "$(printf '%s\n%s\n' "$max" "$floor" | sort -V | tail -n1)" != "$floor" ]; then
    echo "::error::$bin needs glibc $max, above the $floor floor. Symbols over the floor:"
    objdump -T "$bin" | grep -E 'GLIBC_[0-9]' | while read -r line; do
        ver=$(grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' <<<"$line" | sed 's/^GLIBC_//')
        if [ "$(printf '%s\n%s\n' "$ver" "$floor" | sort -V | tail -n1)" != "$floor" ]; then
            echo "  $line"
        fi
    done
    exit 1
fi
