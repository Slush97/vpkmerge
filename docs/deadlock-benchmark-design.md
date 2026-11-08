# Deadlock benchmark map: design

Direction update: use the normal `dl_midtown` map for the main benchmark.
The [Midtown fight probe](../tools/deadlock-benchmark/teamfight/README.md)
is the first implementation trial. It places four scripted heroes ahead of
the current view. Before comparing performance, choose and record a clear
fixed location, verify the attacks and ability effects, and align the capture.
The custom map route below is an earlier proposal; new geometry is deferred.

Status: design with a [development prototype](../tools/deadlock-benchmark/README.md).
The particle-lab generator, Modlock timing probe and capture analyzer are
implemented. The Standard scene is not built. The new lab maps compile with
CSDK 13 and load on the current game server; a client also connected to the
sixteen-emitter map and started its renderer. Visible effects, the fixed view
and capture have not been validated together. There is no benchmark score to
download yet. See the [runtime evidence](../tools/deadlock-benchmark/runtime/README.md)
for the toolchain fix and compatibility checks.

## Goal

Make it easy to compare the effect of a graphics setting, mod, driver or game
update using the same repeatable scene. A run should be understandable to a
player and detailed enough for someone else to reproduce.

The benchmark is a controlled test, not a prediction of FPS in an online
match. A real match has different players, abilities, server conditions and
views. Use recorded gameplay as a second check when a result needs to reflect
actual play.

## What we learned from CS2

[CS2 FPS Benchmark Dust2](https://steamcommunity.com/sharedfiles/filedetails/?id=3240880604)
uses a familiar environment and recreates events such as gunfire, grenades and
smoke. Its creator warns that its local simulation is different from a real
competitive match. It is popular: its Workshop page lists hundreds of
thousands of subscribers. The useful idea is a recognizable, repeatable mix of
work, with a clear warning about what the result means.

The [CS2 consistent benchmarking tool](https://github.com/9Erza/CS2-consistent-benchmarking-tool)
adds external capture to a fixed map so settings can be compared over repeat
runs. We should make the timing boundaries and raw data explicit rather than
depend on an FPS number displayed on screen.

## Player experience

1. Open the benchmark in a local session. Choose **Standard** or **Particle
   lab**. The start screen shows the benchmark version, game build and whether
   the run is supported on that build.
2. Prepare a frame-time capture tool and start the test. The screen explains
   when to begin and end capture. A warm-up plays before the measured run.
3. Watch the same camera route and effects on every run. A small countdown
   marks the beginning; the end marker appears only after measurement ends.
4. Repeat at least three measured runs with the same settings. Save each raw
   capture and its settings. Report the median run and the spread, not only the
   best result.

The first version can use manual capture boundaries. Automatic capture is a
later improvement and must be checked against the same visible markers. A
failed capture or unsupported game build must never produce a success score.

## Standard scene

Target a **60-second measured route**, preceded by a complete unmeasured
warm-up. These times are initial design targets, not measured engine limits.
The route should move smoothly through a compact scene using Deadlock assets:

| Time | View | Intended workload |
| --- | --- | --- |
| 0–15 s | Lane and long street view | Buildings, distance, shadows and common props. |
| 15–30 s | Interior to outdoor transition | Different lighting, visibility and asset streaming. |
| 30–45 s | Street fight | Fixed hero or NPC poses and scheduled weapon and ability effects. |
| 45–60 s | Dense encounter | Several overlapping effects and characters, then a clear ending. |

Keep the camera path, field of view, exposure policy, scene objects, effect
timing and character count fixed for each benchmark version. Use game-time
events where feasible; verify camera and effects remain synchronized with the
capture under low FPS and stalls. Use authored or scripted placements instead
of unpredictable AI. If particle simulation is random or cannot be seeded,
document it and measure how much it varies.

This scene is meant to resemble play, but it will not be called representative
until it is checked against captures from real matches. In particular, a local
server can add CPU work absent from a remote match. Record whether the server
is local or separate and compare only runs with the same arrangement. Do not
add a synthetic overall score until the scene is stable and useful.

## Particle lab

Keep a second, clearly labeled test for diagnosing effects. Use one fixed
camera and a fixed set of effects. Begin with a no-effect baseline and one
modest effect load; add heavier loads only after checking that all effects
actually render and the counts mean what we think they mean. Report each load
separately. It should not contribute to the Standard result.

The existing `particle_bench_0` and `particle_bench_16` maps are useful starting
points. Their compile success is recorded in the [CSDK worklog](../tools/csdk-native/WORKLOG.md),
but the current checkout does not contain the old runner source. The existing
maps are probes, not proof that a current benchmark session works.

## Measurement rules

- Capture per-frame timings throughout the measured interval. On Windows,
  evaluate [PresentMon](https://github.com/GameTechDev/PresentMon/blob/main/README-CaptureApplication.md)
  or a compatible front end. On Linux, evaluate
  [MangoHud](https://github.com/flightlessmango/MangoHud) logging with the
  actual Deadlock/Proton renderer. Verify the selected tool really captures the
  game and records individual frames before using its output.
- State whether the data describes presented frames or displayed frames. Do not
  silently compare these, or data from different capture tools and versions.
  Record frame generation and distinguish generated frames when the capture
  tool supports it. FPS and frame timing do not by themselves measure input
  latency.
- Report average FPS as frame count divided by elapsed capture time. Report the
  99th percentile frame time in milliseconds and a **1% low FPS**, explicitly
  defined as `1000 / mean(slowest 1% of frame times in ms)`, selecting by frame
  count and rounding the count up. Record the sample count. Different tools
  use different low-FPS definitions; do not silently mix them. See
  [CapFrameX's explanation](https://www.capframex.com/blog/post/Explanation%20of%20different%20performance%20metrics).
- Save a frame-time graph and the raw data. Keep long frames in the result;
  do not discard outliers just to improve a score. An interrupted or invalid
  run is marked invalid with a reason, not selectively trimmed.
- Measure at least three complete runs after warm-up. For each metric, show
  all run values, the median and range; do not merge raw frames from separate
  runs. If variation is too large to distinguish the change being tested, run
  more trials and say the comparison is inconclusive. For A/B changes, alternate
  order, such as A-B-B-A, to reduce time and temperature bias.
- Fix and record resolution, window mode, render scale, upscaling, graphics
  preset and overrides, FPS cap, VSync, display refresh, OS, GPU driver, CPU,
  GPU and memory. Record active mods, capture tool/version and server setup.
  Keep the same settings for an A/B comparison. Record shader-cache policy;
  do not clear it between ordinary warm comparisons. Cold-start tests belong
  in their own labeled experiment.

Capture boundaries must be defensible: record the actual start and end markers
in both the scenario and the capture clock, or provide a documented manual
procedure with enough extra capture to align them afterwards. Do not assume
game time and wall time remain identical during pauses or stalls. Reject runs
with missing markers, incomplete intervals, map/load screens in the interval,
missing effects, lost capture data or an unexpected pause. First-frame and
last-frame selection must be documented by the eventual capture adapter.

## Build and runtime design

Keep three responsibilities separate:

1. **Map:** geometry, lighting, cameras and named placements. Author and
   compile with the community Source 2 toolchain. Keep map source and build
   instructions with the project; do not commit extracted retail assets without
   a distribution plan.
2. **Scenario:** a small Modlock project schedules setup, camera movement,
   effects and start/end markers. Prefer named entities and a short readable
   timeline over a general-purpose framework. Prove that the installed host can
   select the map, control the camera and trigger the chosen effects before
   depending on any of those APIs.
3. **Capture and report:** an external adapter records frame times and run
   metadata, checks boundaries and writes results. Keep platform-specific
   capture separate from the metric calculations. Retain the original capture.

Give map, scenario, capture adapter and metric definitions their own version
identifiers. Include the map and scenario content hashes in the result. A
changed route, effect list, camera or timing is a new benchmark version; do
not rank it alongside the old one without re-testing. Store a short manifest
with each run: versions and hashes, game build, timestamps, settings, hardware,
capture source and method, raw capture path, marker alignment, validity and
reason. Missing information should be marked unknown rather than invented.

## First implementation milestones

1. **Compatibility probe:** check the current game build against the staged
   toolchain, then load an existing minimal map in a local client. Verify a
   visible effect and capture real per-frame data. Preserve logs and note exact
   versions. The `csdk-native` stage currently reports build 6819 while the
   installed retail game reports 6860, so freshness must be resolved before
   treating the old stage as current.
2. **Timing probe:** create one fixed camera, one timed event and visible start
   and end markers. Prove capture alignment and repeat it three times. Compare
   the route and effect timing under both high and low frame rates.
3. **Particle lab:** rebuild a minimal, documented runner around the existing
   sources. Verify the zero and modest loads, capture raw frames and calculate
   the stated metrics from a small set of known timing samples.
4. **Standard prototype:** build the four-part route. Verify every scene and
   effect, profile repeatability on more than one machine, and compare against
   real gameplay. Set acceptance thresholds only after observing run-to-run
   variation; publish the measured variation and any known limitations.

Do not call the benchmark production-ready until another person can follow the
instructions, complete three valid runs and reproduce a result within the
variation established by testing. Recheck after game, map, toolchain or capture
changes.
