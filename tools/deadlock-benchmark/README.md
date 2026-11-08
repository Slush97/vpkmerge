# Deadlock benchmark

## Try the Midtown fight

The main experiment now uses the normal map with four scripted Wraith bots.
The custom particle maps are separate diagnostic tests. To open the normal
map and automatically try the fight on this machine:

```sh
python3 tools/deadlock-benchmark/runtime/play_lab.py restart --map midtown --demo
```

This replaces only the launcher's saved session. Face a clear street before
the automatic start, or omit `--demo` and type `/bench_start` in game chat when
ready. `/bench_stop` removes the bots and restores your camera. See the
[fight instructions](teamfight/README.md) for the timing and known limits.
This is a working experiment, not a validated performance score.

## Project status

The [replay-derived fight probe](replay/README.md) can extract a real replay's
combat window and package twelve recorded hero paths and ability timings for
Modlock. It includes a strict equipment-restore mode and an explicit current-build
fallback. Its synthetic gunfire and incomplete world restoration make it an
exploratory workload; it does not produce a validated benchmark score.

A repeatable local scene for comparing graphics settings, mods and game builds.
It measures how quickly the game draws frames. It does not predict the FPS of
an online match or measure input latency.

This is a development prototype. A successful build or analysis test does not
establish that the map renders correctly. The intended finished benchmark is
described in the [design](../../docs/deadlock-benchmark-design.md).

Implemented: lab map source generation, a Modlock timing probe, capture
validation, frame-time graphs and repeated-run reporting. The four-part
Standard scene is not built. Both new lab maps compile with the coherent
CSDK 13 toolchain and load on the current game server. An isolated client
connected to the sixteen-emitter map and created a renderer. The old mixed
toolchain's ConVar registration failure is resolved by using CSDK 13 for builds.
Visible effects, the fixed camera and a complete measured run remain unverified.
See [runtime results](runtime/README.md) for the evidence and exact limits.
A real MangoHud capture is retained there as well. It has no aligned scenario
markers or verified view, so it is not a scored benchmark run.

## Project layout

| Folder | Purpose |
| --- | --- |
| `map/` | Generate map sources from a local template. |
| `scenario/` | Control the test and mark its start and end with Modlock. |
| `teamfight/` | Script a small encounter on the normal Midtown map. |
| `runtime/` | Check the local installation and run isolated compatibility probes. |
| `analysis/` | Validate frame captures and produce results and a frame-time graph. |
| `tests/` | Check measurement rules using known sample data. |

## Run a test

Begin with the [runtime check](runtime/README.md), then follow the
[map](map/README.md), [scenario](scenario/README.md) and
[capture](analysis/README.md) instructions. Keep generated maps,
build output and captures outside the source tree. Use a new output directory
for each run so an old result cannot be mistaken for a new one.

Start with the Particle Lab baseline and modest load. Check the view and the
effects before recording results. Warm up the scene, then capture the complete
measured interval. The scenario clock and the capture clock are different;
align their start and end markers using the procedure in `analysis/README.md`.

Capture at least three runs with the same settings. Retain the original CSVs
and fill in each run's manifest. The analysis tool refuses a scored result
when required checks are missing. These checks are statements by the operator;
the tool cannot prove that a particle was visible or the game kept focus.

Do not compare different map versions, capture methods or workloads as though
they were the same test. Change one setting at a time. Alternate test order
when comparing two configurations, and keep the results from every valid run.

## Read the results

- **Average FPS:** frames presented during the measured time, divided by that
  time. Higher is better.
- **1% low FPS:** the average duration of the slowest one percent of frames,
  converted to FPS. Higher is better. This is not the minimum FPS.
- **99th percentile frame time:** the time that 99 percent of frames meet or
  beat, in milliseconds. Lower is better.
- **Median and range:** the middle result and lowest-to-highest result across
  repeated runs. A change smaller than normal variation is inconclusive.

Read the frame-time graph as well as the averages. A spike is a long frame.
Keep those frames in the result unless the entire run was invalid for a
documented reason.

## Development

Run the measurement tests from the repository root:

```sh
python3 -m unittest discover -s tools/deadlock-benchmark/tests -v
```

Build the scenario from `tools/deadlock-benchmark/scenario` with `modlock build`.
This checks the script; it does not launch a client.

Keep map generation, game control and analysis separate. Add an adapter only
after checking its real input format. Record unknown information as unknown.
Do not turn a missing measurement into zero or a failed run into a score.

Generated files and test samples are not benchmark evidence. A release needs
current-game map loading, verified camera and effects, aligned frame capture,
and repeated runs on more than one machine. The [design](../../docs/deadlock-benchmark-design.md)
defines the remaining acceptance checks.
