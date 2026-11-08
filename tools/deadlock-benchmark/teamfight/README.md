# Midtown fight probe

This early experiment uses the ordinary `dl_midtown` map and four Wraith bots,
two on each team. It issues normal weapon and ability input through Modlock;
this is a scripted encounter, not a replay or a stable representation of an
online match. Bot movement, damage, deaths, cooldowns, other game entities,
particle behavior and frame timing can vary. Bot damage is capped to one
per hit to keep the encounter going; this changes gameplay and workload. No game build is certified yet.

Build with `modlock build` in this directory and use a local Modlock session
with `dl_midtown` loaded. Spawn a hero, face down a wide, clear area away from
other units and walls, and type `/bench_start`. A session launched with `--arg demo` makes one
automatic attempt five seconds after a human and their camera are ready. In
demo mode the fight keeps running after the measured pass until `/bench_stop`;
that extra time is not a capture interval. The current camera becomes the
fixed viewpoint, with the bots centered 500 units ahead in the view direction.
The server log records the anchor and camera coordinates. A floor trace checks
bot positions when available; otherwise the probe logs a fallback to the
operator’s height. Neither guarantees clear sightlines or camera placement.
Check that all four bots appear, face and shoot each other, and show ability
effects. A successful command or bot input does not prove a shot or cast
occurred. The log records setup, ability names, accepted inputs, observed
movement cast events and damage events. Zero or missing casts or hits means
this encounter was not verified.

After eight seconds of setup the fight runs a 60-second warm-up, followed by a
60-second measured pass. The same input cycle is used in both: ability-one taps
every four game seconds, primary fire and periodic reload, with fixed-position
resets. During warm-up, start a frame-time capture; stop only after the visible
`BENCH END`. `/bench_status` reports the current phase, and `/bench_stop`
cancels the test, removes the bots and restores the camera. The run aborts if
setup, bot input, actor health or camera requests fail, if the operator leaves,
or if the world changes. The game can move or kill actors between checks.

Keep the capture clock independent from the game clock. Record the capture-clock
times of both visible markers using a synchronized recording or another
verified alignment method. Reject a run for missing markers, pauses, camera
movement, missing bots/effects, absent captures or incomplete frames. Preserve
raw capture and logs, map/game/Modlock versions, coordinates, graphics and
hardware settings. Compare only runs with identical anchor, map, scenario and
settings; three or more valid runs are needed to assess variation. The probe
has not yet demonstrated repeatability, verified capture alignment or a valid
score. The separate `scenario/` project remains the particle-lab probe.
