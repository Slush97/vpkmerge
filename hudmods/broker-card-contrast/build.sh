#!/usr/bin/env bash
# Rebuilds the override from the live game's copy of the stylesheet so a Valve
# patch to it is picked up instead of reverted.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
pak=${1:-${DEADLOCK_PAK:?pass citadel/pak01_dir.vpk or set DEADLOCK_PAK}}
out=${2:-$here/target/broker_card_contrast_dir.vpk}
entry=panorama/styles/tooltips/citadel_mod_tooltip_shared.vcss

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

vpkmerge() { cargo run -q --release --manifest-path "$repo/Cargo.toml" -p vpkmerge-cli -- "$@"; }

vpkmerge panorama dump --vpk "$pak" --out-dir "$work" --prefix "${entry}_c" >/dev/null
[[ -s "$work/$entry" ]] || { echo "could not extract readable $entry from $pak" >&2; exit 1; }
cat "$here/corrupted_contrast.vcss" >>"$work/$entry"
mkdir -p "$(dirname "$out")"
vpkmerge panorama build --workspace "$work" --output "$out"
