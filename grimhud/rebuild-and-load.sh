#!/usr/bin/env bash
# Rebuild the grimhud Panorama addon VPK and load it straight into Grimoire as a
# named local mod with a thumbnail. One command from edited .vjs/.vcss source to
# "showing in Grimoire with art".
#
# Steps:
#   1. compile + pack  -> target/grimhud_dir.vpk   (build_panorama_addon.py)
#   2. import          -> Grimoire local mod        (grimoire/scripts/import-local-mod.mjs)
#
# It always loads into the SAME fixed addons slot (default pak02; override with
# --slot N or GRIMHUD_SLOT), overwriting in place, so re-running on every edit
# never accumulates duplicate paks.
#
# NOTE: this does NOT re-derive layout/hud.vxml. Only re-derive that (see
# HANDOFF.md "Rebuild + reinstall") when you ADD or REMOVE a script/style file.
#
# usage: grimhud/rebuild-and-load.sh [--name NAME] [--slot N] [--no-import]
set -euo pipefail

NAME="GRIMHUD (dev)"
CATEGORY="HUD"
SLOT="${GRIMHUD_SLOT:-2}"
DO_IMPORT=1
while [ $# -gt 0 ]; do
  case "$1" in
    --name) NAME="$2"; shift 2 ;;
    --slot) SLOT="$2"; shift 2 ;;
    --no-import) DO_IMPORT=0; shift ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VPK_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
GRIMOIRE_DIR="$(cd "$VPK_ROOT/../grimoire" && pwd)"
OUT_VPK="$VPK_ROOT/target/grimhud_dir.vpk"
THUMB="$SCRIPT_DIR/grimhud_thumb.png"

cd "$VPK_ROOT"

echo "==> compiling + packing grimhud addon"
python3 tools/panorama-compiler/build_panorama_addon.py grimhud/src \
  --addon grimhud --output "$OUT_VPK" --force

if [ "$DO_IMPORT" -eq 0 ]; then
  echo "built $OUT_VPK (skipped import)"
  exit 0
fi

echo "==> importing into Grimoire as \"$NAME\""
IMG_ARGS=()
[ -f "$THUMB" ] && IMG_ARGS=(--image "$THUMB")   # else the importer auto-generates a card
node "$GRIMOIRE_DIR/scripts/import-local-mod.mjs" "$OUT_VPK" \
  --name "$NAME" --category "$CATEGORY" --slot "$SLOT" "${IMG_ARGS[@]}" --force

echo "done. Reopen Grimoire (or refresh) to see it."
