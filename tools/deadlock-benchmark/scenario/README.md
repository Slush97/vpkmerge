# Particle lab timing probe

This experimental Modlock scenario is not a finished benchmark. It requests one
fixed free camera and runs 30 seconds of warm-up followed by 60 seconds of
game-time measurement. It needs a compatible server and client with one of the
generated maps loaded. Map selection, camera placement, effects, HUD and timing
have not been checked together in a real session. `freeCamera` can still be
moved by the player; do not touch movement controls. A successful API return
does not prove the camera was actually placed or held. The Standard route is
not built. Particle emitters are continuous; their simulation is not seeded or
reset between warm-up and measurement, and variation is not yet measured.

Build from this directory with `modlock build`. Start the mod using your local
Modlock workflow after setting up a compatible isolated stage; manually load
`dl_benchmark_lab_0` or `dl_benchmark_lab_16`. Type `/bench_start` in chat.
Start frame-time capture during warm-up, before the visible `BENCH START`, and
stop after `BENCH END`. Save logs and a screen recording with a timestamped
clock shared with the capture, or use another verified method that aligns both
visible markers to the capture clock. Do not infer capture-clock boundaries
from the game timer. Record actual capture times and alignment method in the
run manifest. If markers, camera, effect visibility, clock alignment or
complete per-frame capture cannot be verified, the run is invalid. A world change or operator disconnect invalidates an active run; a pause or missing effect also invalidates it
even if the script reaches END. Retain the full raw capture.

Record the version in `mod.json`, source hash, map source and compiled hashes,
game build, Modlock version, and all measurement settings. Fixed coordinates
come from the historical CSDK runner; their interpretation and camera angles
in this host remain unverified. Wait for the full warm-up on every run. Test
baseline and 16 emitters separately; never combine their results.
