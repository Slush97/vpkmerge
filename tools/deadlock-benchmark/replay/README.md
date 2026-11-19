# Replay-derived fight probe

Extract a real fight's hero paths, equipment snapshots and ability events,
then play an experimental reconstruction on Midtown through Modlock. This
is a development tool, **not a validated benchmark or an exact replay**.

## Current replay: Night Shift APAC, 132204644

The November 6 OT–PIG8 replay uses client version 6860, matching the installed
client. Its selected 30-second fight plus five-second lead-in packages twelve
actors and 88 inferred hero ability events. Eleven duplicate samples differ
only in field observation timestamps. One dead pawn has 920 sparse ticks held
between confirmed dead states, including a parser snapshot at the clip end;
corpse movement is not reconstructed. Living-hero tracks require no interpolation.

Native playback of the selected fight passed. The Grimoire launch button also
completed a warm-up and three passes without team selection, then automatically
closed its client and server. The adapter/runtime tests, Go vet and Modlock
build pass. The earlier generated package was byte-repeatable, but runtime
cast counts and poses vary: this is not a validated performance benchmark. See
[validation evidence](../runtime/evidence/replay-probe-132204644.json).

The prepared project is
`~/.cache/deadlock-benchmark-replay/night-shift-20261106/132204644.scenario-v7`.
It runs one full warm-up and three measured passes, resetting actors between
passes. Start it with the isolated launcher after closing the retail client.
`--supervise` closes both processes after all passes. Manual Stop uses the same
state directory:

```sh
python3 tools/deadlock-benchmark/runtime/play_lab.py start \
  --game /tmp/deadlock-benchmark-replay-game \
  --scenario "$HOME/.cache/deadlock-benchmark-replay/night-shift-20261106/132204644.scenario-v7" \
  --state-dir "$HOME/.cache/deadlock-benchmark-night-shift-play" --demo --supervise
```

For an initial suite, use three distinct 30-second scenes, one warm-up per scene
and three measured repetitions per scene. Five-second lead-ins and actor resets
bring the expected runtime to roughly 8–10 minutes, or about three minutes for
one scene. This is a starting design: use observed variation to decide whether
longer clips or more repetitions are needed. Actor reset does not reset all
world state or establish deterministic simulation. Native replay playback and
Modlock reconstruction must be evaluated separately.

The launcher defaults to the installed game's video settings. For a fixed
window, pass `--resolution 1920x1080` (also 1280x720 or 2560x1440). Each session
saves `video-settings.txt` and `display.json`. Render scale and upscaling are
preserved, so window size alone does not define internal render resolution.
See the sibling Grimoire `docs/benchmark.md` for the launcher UI and upstream
Modlock/Hyperline setup review.

## Earlier trial: incompatible replay 107073558

The local `107073558.dem` decoded successfully. The selected 30-second window
contained 776 nearby opposing-hero damage events involving all twelve players.
With five seconds of lead-in, its package contains 55 scheduled hero ability
events and twelve 64 Hz tracks. Four missing ticks per actor were filled;
483 initially unattributed samples matched an initial pawn's index and serial.

Native replay playback failed during sign-on with serializer mismatches.
Strict hero restoration also failed because an old ability definition no longer
matched its slot. The explicit current-build fallback completed a full warm-up
and a 30-second test interval with the native chase camera. Earlier roaming
camera probes crashed in `client.dll`; the underlying cause is not established.

The final pass had 55 scheduled hero casts versus 38 observed hero casts (37
in warm-up); these are aggregate counts, not a one-to-one match. Maximum sampled
position difference reached 593.2 units during combat. Screenshots show actors
and effects, but also obstructed or clipped views. The captured frame files
are not aligned to verified visible markers. **No FPS score or live-match
equivalence is established.** The remaining work needs a compatible current
replay, better cast reproduction, world-state restoration and capture validation.

See the [machine-readable result](../runtime/evidence/replay-probe-107073558.json).
Raw data, a ready-built scenario, screenshots and logs are preserved locally in
`~/.cache/deadlock-benchmark-replay/107073558-20261107/`. To replay that exploratory
scenario on this machine:

```sh
python3 tools/deadlock-benchmark/runtime/play_lab.py start \
  --game /tmp/deadlock-benchmark-replay-game \
  --scenario "$HOME/.cache/deadlock-benchmark-replay/107073558-20261107/scenario" \
  --state-dir "$HOME/.cache/deadlock-benchmark-replay-play" --demo
```

The trial session was stopped after preserving its results. The saved scenario
uses the current-build fallback with synthetic gunfire and no recorded items.

## Extract another replay

The extractor pins [s2replay](https://github.com/paralin/s2replay) to
`aa8bed7a353c68fd53a8e57a88988ea51d307867`. Build it with Go 1.25 or later:

```sh
cd tools/deadlock-benchmark/replay/extractor
go build -o /tmp/replay-extractor .
```

From the repository root, scan a local replay and select a 30-second fight:

```sh
/tmp/replay-extractor -mode scan -demo /path/to/match.dem -out /tmp/fight-scan
python3 tools/deadlock-benchmark/replay/prepare.py select /tmp/fight-scan --output /tmp/fight-selection.json
```

The selector ranks spatial clusters of opposing-hero damage by participant
count, event count and total damage, using a 2,500-unit radius and a five-second
search grid. It samples up to roughly ten anchors per window; it is a heuristic,
not a definition of every teamfight. `start_tick` includes five seconds of
lead-in. Times here are demo ticks, not the displayed match clock.

Use the printed start and end ticks to extract the clip. For the locally tested
`107073558.dem`, they are 120000 and 122240:

```sh
/tmp/replay-extractor -mode clip -demo /path/to/match.dem -start 120000 -end 122240 -out /tmp/fight-clip
python3 tools/deadlock-benchmark/replay/prepare.py package /tmp/fight-clip --selection /tmp/fight-selection.json --output /tmp/fight-scenario --repetitions 3
modlock build /tmp/fight-scenario
python3 tools/deadlock-benchmark/runtime/play_lab.py start --game /tmp/fight-game --scenario /tmp/fight-scenario --state-dir /tmp/fight-session --demo
```

Output paths must be new. Keep the original replay, extraction output and
generated project. The raw clip is about 73 MB for this recording; extracting
its initial world snapshot briefly used about 9 GB of RAM. The source replay
is read only. The launcher uses a separate game copy; inspect its inherited
configuration and asset search paths before any performance comparison.

The scenario moves the observer into spectator mode, spawns all twelve replay
bots, restores their builds, warms up by playing the full clip, then restores the
same bots and plays three lead-ins plus measured intervals. Keeping controller
slots stable avoids the client crash seen when recreating the whole roster. `/bench_start` starts a
manual run; `/bench_stop` removes the actors and releases the camera;
`/bench_status` reports progress. Stopping releases the camera and leaves the
observer a spectator. Stop the session with the same `--state-dir`:

```sh
python3 tools/deadlock-benchmark/runtime/play_lab.py stop --state-dir /tmp/fight-session
```

Playback uses `frame.timeSeconds`, the same clock the SDK uses to advance
`Puppet`. The scoreboard's `matchClock()` is only a display clock. Each reused
puppet is paused, rewound and primed on the first frame before playback resumes;
`seek(0)` alone does not reset its clock. The scenario checks tick alignment,
including the clamped final sample. Reproduce the original one-tick reset error
against a local upstream SDK with:

```sh
bun tools/deadlock-benchmark/analysis/probe-puppet-clock.ts /path/to/modlock/js/src
```

The current 30-second measurement has five seconds of lead-in and about nine
seconds of restoration between passes, so visible rewinds occur about 44 seconds
apart. Restoration is a separate phase, not uninterrupted gameplay. `update_gap`
and `update_summary` logs distinguish delivered simulation-update gaps from
rendered frame times; they include lead-in, exclude restoration, and count
simulation gaps over 1.5 ticks and wall-clock gaps over 50 ms. Detailed gap logs
are bounded; summary counters cover the whole pass. Wall time uses `Date.now()`
and is diagnostic only, not a monotonic clock or validated capture alignment.
Smooth client frame delivery does not establish smooth simulation. These source
changes require repackaging a fixture; existing Grimoire bundles are not updated
automatically.

For old replays whose equipment no longer restores, package a separate project
with `--adapt-current-build`. That explicit fallback uses today's hero abilities
by slot, upgrades them, and omits recorded items. Its logs identify the
substitutions. It is useful for checking the pipeline and viewing motion, but
changes the workload substantially. Strict mode aborts on a failed restore.
Add `--motion-only` to isolate recorded paths without combat or recorded
health/deaths. This is a separate diagnostic workload.

## What the prototype reproduces

- Hero identity and team, 64 Hz recorded positions, facing and health. Gaps of
  up to four ticks are filled and counted. For confirmed dead pawns only, gaps
  of at most 128 ticks hold the previous pose and are counted separately. A
  missing dead-pawn end boundary requires a matching final parser snapshot.
  Other missing boundaries, larger gaps, conflicting identities and missing
  required fields are refused. Unattributed
  samples can use an exact entity-index/serial match to the initial snapshot.
- Hero ability input scheduled from observed cooldown transitions. Slot zero
  is preserved even though the parser omits its zero-valued JSON field. Input
  acceptance does not establish that a cast happened; inspect observed casts.
- A native chase camera following the continuously alive hero whose average
  position is closest to the fight anchor. It needs visual inspection.

The recording has no `DEM_UserCmd` stream. **Gunfire is synthetic**, holding
primary fire with periodic reload. Missed shots, exact aim inputs, movement
commands, targeting and cast releases are not reconstructed. Puppet motion is
forced; health follows the recording and live damage to replay heroes is
suppressed. Shields, active modifiers, exact cooldowns, trooper waves, destroyed
objectives and other world state are not restored. Ability cooldowns are
refreshed before scheduled attempts. Current hero definitions and map geometry
can differ from the source. These are material performance differences.

`valid_benchmark` remains false in the package, regardless of whether playback
finishes. Do not turn its logs or a successful build into a performance score.
For measurement, use the existing [capture procedure](../analysis/README.md),
verify the camera and effects, independently align both markers, and retain
at least three runs. A local server also consumes CPU on the measured machine.
Original replay playback and live reconstruction are different workloads.

Run the adapter tests with the existing benchmark tests:

```sh
python3 -m unittest discover -s tools/deadlock-benchmark/tests -v
```
