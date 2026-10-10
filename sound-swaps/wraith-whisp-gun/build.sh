#!/usr/bin/env bash
# Build the Wraith whispy/futuristic weapon-sound swap into one addon VPK:
# fire (first + main), bullet whizby, and the full reload sequence.
# Re-run after editing synth_whisp_gun.py to iterate on the sound design.
set -euo pipefail
cd "$(dirname "$0")"

PAK="${DEADLOCK_PAK:-$HOME/.steam/steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk}"
VPKMERGE="../../target/release/vpkmerge"

# event name -> synth file stem
EVENTS=(
  "Wraith.Wpn.Fire.First=fire_first"
  "Wraith.Wpn.Fire.Main=fire_main"
  "Wraith.Wpn.Whizby=whizby"
  "Wraith.Wpn.Reload.Start=reload_start"
  "Wraith.Wpn.Reload.Clip.Out=reload_clip_out"
  "Wraith.Wpn.Reload.Clip.In=reload_clip_in"
  "Wraith.Wpn.Reload.End=reload_end"
)

echo "== synth =="
python3 synth_whisp_gun.py

echo "== encode mp3 (clean CBR mono, no Xing/metadata) + swap each event =="
PARTS=()
for pair in "${EVENTS[@]}"; do
  ev="${pair%%=*}"; f="${pair##*=}"
  ffmpeg -y -loglevel error -i "$f.wav" -ac 1 -ar 44100 \
    -c:a libmp3lame -b:a 192k -map_metadata -1 -write_xing 0 "$f.mp3"
  # --pool all: override every clip in the event's pool in place; .vsndevts_c untouched
  "$VPKMERGE" soundswap --from-vpk "$PAK" --event "$ev" --hero wraith \
    --audio "$f.mp3" --pool all --encode-vpk "${f}_dir.vpk" >/dev/null
  echo "  $ev <- $f.mp3"
  PARTS+=("${f}_dir.vpk")
done

echo "== merge all into one addon =="
"$VPKMERGE" wraith_whisp_gun_dir.vpk "${PARTS[@]}"

echo
echo "built: wraith_whisp_gun_dir.vpk"
echo "audition raw sounds: play *.mp3"
