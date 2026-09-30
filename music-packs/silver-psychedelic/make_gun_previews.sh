#!/usr/bin/env bash
# Render 60s-themed gunfire + cast-sting candidates from the vanilla werewolf
# rifle shot, plus cadence demos (0.85s normal spacing + a slamfire burst) so
# the sound can be judged the way it plays. Pure ffmpeg, no new sources.
set -euo pipefail
SRC=/tmp/gun_src
SFX="$(dirname "$0")/source_audio"
OUT="$(dirname "$0")/previews"
mkdir -p "$OUT"

# Tight single gunshot base: trim to the transient + short tail, peak-normalize.
base="$SRC/vanilla_fire_01.mp3"
ffmpeg -v error -y -i "$base" -t 0.75 -af "afade=t=out:st=0.55:d=0.2,dynaudnorm=p=0.9" "$OUT/_dry.wav"

# --- Style filter chains (each keeps a real gun, adds 60s character) ---
declare -A STYLE
# Spaghetti-western: slapback + canyon reverb taps + slight shimmer.
STYLE[western]="aecho=0.9:0.55:55|140|280:0.5|0.32|0.18,aecho=0.8:0.5:430:0.22,treble=g=3"
# Fuzz pedal (Hendrix): hard drive into a limiter, phaser movement, body shaping.
STYLE[fuzz]="highpass=f=120,volume=14dB,alimiter=limit=0.5,volume=-5dB,aphaser=type=t:speed=1.5,lowpass=f=9000"
# Surf rock: boingy spring reverb + fast tremolo.
STYLE[surf]="aecho=0.85:0.7:32|64:0.6|0.4,tremolo=f=9:d=0.55,treble=g=2"

for name in western fuzz surf; do
  ffmpeg -v error -y -i "$OUT/_dry.wav" -af "${STYLE[$name]},alimiter=limit=0.95" "$OUT/gun_${name}.wav"
done

# Ricochet variant: western shot + a descending chirp ("pyew") tail mixed in.
ffmpeg -v error -y -f lavfi -i "aevalsrc='0.25*sin(2*PI*(2600*t-2000*t*t))':d=0.5:s=44100" \
  -af "afade=t=in:st=0:d=0.02,afade=t=out:st=0.3:d=0.2,aformat=channel_layouts=stereo" "$OUT/_chirp.wav"
ffmpeg -v error -y -i "$OUT/gun_western.wav" -i "$OUT/_chirp.wav" \
  -filter_complex "[1]adelay=70|70[r];[0][r]amix=inputs=2:weights=1 0.8:normalize=0,alimiter=limit=0.95" "$OUT/gun_ricochet.wav"

# --- Cast / activation sting: reverse-cymbal swell -> fuzzed gunshot accent ---
# Take ~0.6s of reverse cymbal (the build), then hit the fuzzed shot on the downbeat.
ffmpeg -v error -y -i "$SFX/rev_cymbal_short.wav" -t 0.6 -af "dynaudnorm=p=0.9,afade=t=in:st=0:d=0.1" "$OUT/_swell.wav"
ffmpeg -v error -y -i "$OUT/gun_fuzz.wav" -i "$SFX/organ_stab.wav" \
  -filter_complex "[1]atrim=0:0.5,asetpts=PTS-STARTPTS,volume=-4dB[o];[0][o]amix=inputs=2:weights=1 0.6:normalize=0[hit]" \
  -map "[hit]" "$OUT/_hit.wav"
# Concatenate swell then hit (swell tail overlaps the hit slightly).
ffmpeg -v error -y -i "$OUT/_swell.wav" -i "$OUT/_hit.wav" \
  -filter_complex "[0]adelay=0|0[a];[1]adelay=480|480[b];[a][b]amix=inputs=2:weights=0.8 1:normalize=0,alimiter=limit=0.95" "$OUT/cast_sting.wav"

# --- Cadence demos: 4 shots at 0.85s, then a slamfire burst at 0.32s ---
make_cadence() {
  local shot="$1" name="$2"
  # normal: 4 shots spaced 850ms
  ffmpeg -v error -y -i "$shot" -i "$shot" -i "$shot" -i "$shot" \
    -filter_complex "[0]adelay=0|0[a];[1]adelay=850|850[b];[2]adelay=1700|1700[c];[3]adelay=2550|2550[d];[a][b][c][d]amix=inputs=4:normalize=0,alimiter=limit=0.95" \
    "$OUT/demo_${name}_normal.wav"
  # slamfire: 8 shots spaced 320ms
  local inputs=() fc="" mixin=""
  for i in $(seq 0 7); do inputs+=(-i "$shot"); fc+="[$i]adelay=$((i*320))|$((i*320))[s$i];"; mixin+="[s$i]"; done
  ffmpeg -v error -y "${inputs[@]}" -filter_complex "${fc}${mixin}amix=inputs=8:normalize=0,alimiter=limit=0.95" "$OUT/demo_${name}_slamfire.wav"
}
make_cadence "$OUT/gun_western.wav" western
make_cadence "$OUT/gun_fuzz.wav" fuzz

rm -f "$OUT"/_*.wav
echo "previews in: $OUT"
ls -1 "$OUT"
