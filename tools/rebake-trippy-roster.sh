#!/usr/bin/env bash
# Re-bake the whole-roster trippy ability-VFX merge from the CURRENT base pak01
# and install it as one addon VPK. Run after every Deadlock update: a stale bake
# overrides updated base files with pre-update copies, and bakes made before the
# shape-safety fixes square off shaped textures (see
# memory: squared-off-vfx-shaped-textures).
#
# Usage: tools/rebake-trippy-roster.sh [OUT_dir.vpk]
#   DEADLOCK_PAK01 overrides the base pak path.
set -euo pipefail

PAK="${DEADLOCK_PAK01:-$HOME/.local/share/Steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk}"
OUT="${1:-$HOME/.local/share/Steam/steamapps/common/Deadlock/game/citadel/grimoire/pak03_dir.vpk}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/vpkmerge"
WORK="$(mktemp -d /tmp/trippy-roster.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

[ -f "$PAK" ] || { echo "base pak not found: $PAK" >&2; exit 1; }
[ -x "$BIN" ] || cargo build --release -p vpkmerge-cli --manifest-path "$ROOT/Cargo.toml"

# Pinned roster codenames (mirrors vpkmerge_core::pinned_hero_codenames).
HEROES=(
  bookworm necro yamato chrono abrams archer astro bebop digger doorman
  drifter dynamo familiar fencer frank ghost nano haze hornet kelvin lash
  mcginnis magician mirage pocket priest punkgoat shiv tengu unicorn
  gigawatt vampirebat viper viscous warden werewolf wraith inferno
)
STYLES=(confetti liquid moire kaleido holo glitch thermal gradient camo carbon galaxy halftone lava vaporwave)

INPUTS=()
i=0
for hero in "${HEROES[@]}"; do
  style="${STYLES[$((i % ${#STYLES[@]}))]}"
  out="$WORK/${hero}_dir.vpk"
  echo "== $hero ($style)"
  "$BIN" trippy-vfx --hero "$hero" --vpk "$PAK" --style "$style" \
    --encode-vpk "$out"
  INPUTS+=("$out")
  i=$((i + 1))
done

if [ -f "$OUT" ]; then
  cp "$OUT" "$ROOT/target/$(basename "$OUT" .vpk).pre-rebake.vpk"
  echo "backed up previous bake to target/$(basename "$OUT" .vpk).pre-rebake.vpk"
fi
"$BIN" "$OUT" "${INPUTS[@]}"
echo "installed roster trippy-vfx merge: $OUT"
