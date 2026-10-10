#!/usr/bin/env bash
# Build the final 60s-western gunfire clip set from the vanilla werewolf rifle
# shots, auto-leveled to vanilla loudness (mean ~-14 dB, peaks limited), 44.1k
# stereo. Output feeds bake_silver_gunfire.rs.
set -euo pipefail
SRC=/tmp/gun_src
SFX="$(dirname "$0")/source_audio"
PREV="$(dirname "$0")/previews"
OUT=/tmp/silver_gun_clips
mkdir -p "$OUT"

WESTERN="aecho=0.9:0.55:55|140|280:0.5|0.32|0.18,aecho=0.8:0.5:430:0.22,treble=g=3"
TARGET=-14   # match vanilla rifle mean loudness

# Level a file: gain so mean_volume hits TARGET, then peak-limit, to 44.1k stereo.
level() {
  local in="$1" out="$2"
  local m g
  m=$(ffmpeg -hide_banner -i "$in" -af volumedetect -f null /dev/null 2>&1 \
        | grep -oE 'mean_volume: [-0-9.]+' | grep -oE '[-0-9.]+$')
  g=$(python3 -c "print(round($TARGET-($m),2))")
  ffmpeg -v error -y -i "$in" \
    -af "volume=${g}dB,alimiter=limit=0.9,aformat=sample_rates=44100:channel_layouts=stereo" "$out"
}

# western normal take: <src.mp3> <trim_secs> <out_name>
western() {
  ffmpeg -v error -y -i "$SRC/$1" -t "$2" -af "$WESTERN,afade=t=out:st=$(python3 -c "print($2-0.2)"):d=0.2" "$OUT/_tmp.wav"
  level "$OUT/_tmp.wav" "$OUT/$3.wav"
}

# 3 normal western variants (different vanilla takes -> natural variation)
western vanilla_fire_01.mp3 0.95 western_a
western vanilla_fire_03.mp3 0.95 western_b
western vanilla_fire_04.mp3 0.95 western_c
# bigger first-shot accent (more tail)
western vanilla_first_01.mp3 1.15 first
# tight slamfire takes (short tail for the rapid unload)
western vanilla_fire_02.mp3 0.45 slam_a
western vanilla_fire_03.mp3 0.45 slam_b

# rare ricochet flourish: western shot + descending chirp
ffmpeg -v error -y -f lavfi -i "aevalsrc='0.25*sin(2*PI*(2600*t-2000*t*t))':d=0.5:s=44100" \
  -af "afade=t=in:st=0:d=0.02,afade=t=out:st=0.3:d=0.2,aformat=channel_layouts=stereo" "$OUT/_chirp.wav"
ffmpeg -v error -y -i "$SRC/vanilla_fire_02.mp3" -t 1.0 -af "$WESTERN" "$OUT/_rshot.wav"
ffmpeg -v error -y -i "$OUT/_rshot.wav" -i "$OUT/_chirp.wav" \
  -filter_complex "[1]adelay=70|70[r];[0][r]amix=inputs=2:weights=1 0.8:normalize=0" "$OUT/_ric.wav"
level "$OUT/_ric.wav" "$OUT/ricochet.wav"

# cast sting (already designed) -> re-level
level "$PREV/cast_sting.wav" "$OUT/cast.wav"

rm -f "$OUT"/_*.wav
echo "clips -> $OUT"
for f in "$OUT"/*.wav; do
  d=$(ffprobe -v error -show_entries format=duration -of default=nk=1:nw=1 "$f")
  m=$(ffmpeg -hide_banner -i "$f" -af volumedetect -f null /dev/null 2>&1 | grep -oE '(mean|max)_volume: [-0-9.]+ dB' | tr '\n' ' ')
  printf "  %-14s %4.2fs  %s\n" "$(basename "$f")" "$d" "$m"
done
