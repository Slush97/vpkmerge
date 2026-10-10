#!/usr/bin/env bash
# make-face-video.sh: put your own video on RUINER Billy's helmet screen
# (or any flipbook-screen mod with the same contract).
#
# Extracts frames with ffmpeg, tiles them into the mod's flipbook sheet,
# retimes the material's playback expression, and packs an addon VPK that
# installs alongside the base mod at higher priority.
#
# usage: tools/make-face-video.sh <video> <mod_dir.vpk> <out_dir.vpk> \
#          [fps=24] [grid=32] [maps=shop]
#
# Frame budget is grid*grid; at grid=32 that is 1024 frames (42.7s at 24fps).
# Longer inputs are cut at the budget. maps: shop | all | none (which nested
# UI-map VPKs to patch so the shop/postgame scenes play the new video).
set -euo pipefail

VIDEO=${1:?video file}
MOD=${2:?mod _dir.vpk}
OUT=${3:?output _dir.vpk}
FPS=${4:-24}
GRID=${5:-32}
MAPS=${6:-shop}

# Cell size assumes the RUINER Billy 8192 sheet; the Rust side re-derives the
# real cell from the donor texture and resizes, so this only sets ffmpeg's
# output resolution.
CELL=$((8192 / GRID))
MAX=$((GRID * GRID))

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

ffmpeg -v error -i "$VIDEO" \
  -vf "fps=$FPS,scale=$CELL:$CELL:force_original_aspect_ratio=increase,crop=$CELL:$CELL" \
  -frames:v "$MAX" "$TMP/f_%05d.png"

N=$(find "$TMP" -name 'f_*.png' | wc -l)
echo "extracted $N frames @ ${FPS}fps, cell ${CELL}px (budget $MAX)"

cargo run --release -p vpkmerge-core --example face_video -- \
  --mod "$MOD" --frames "$TMP" --out "$OUT" \
  --fps "$FPS" --grid "$GRID" --maps "$MAPS"
