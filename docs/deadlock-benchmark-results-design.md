# Benchmark research and results design

## Personal and community aggregation prototype

The shared aggregation contract and executable prototype now live in
[`grimoire-social/docs/benchmark-aggregation.md`](../../grimoire-social/docs/benchmark-aggregation.md).
The selected sharing model is local private history, explicit anonymous
community contribution, and separately enabled named profiles. The prototype
uses the eight-attempt config comparison below, preserves its failure and
provisional status, and produces two personal groups and no community groups.
No upload endpoint or production site/app history view has been enabled.

Hardware, OS, preset/settings, workload, runtime, capture semantics and protocol
are group dimensions. Complete launch-policy recording and effective renderer
verification remain explicit gaps. The
[`export-aggregation-pilot.py`](../tools/deadlock-benchmark/analysis/export-aggregation-pilot.py)
adapter verifies the ledger's registered capture and treatment data before
creating a local allowlisted contribution prototype.

## Sqooky Default vs OptiLock FPS, November 7, 2026

On the Ryzen 7 9800X3D / RTX 5070 Ti Linux/Proton machine, the main configs
produced essentially equal average FPS in this exploratory reconstruction.
Both used configured 2560×1440, 100% render scale, no upscaling, no frame cap,
VSync off, and native aspect ratio. Other visual settings differed. Grimoire's
config filtering was applied; VPK add-on bundles were not installed.

| Main config | Completed / attempted | Median average FPS | Median 1% low FPS | Average FPS range |
| --- | ---: | ---: | ---: | ---: |
| Sqooky Default, snapshot `1dbf6794` | 3 / 4 | 297.9 | 159.3 | 297.1–298.4 |
| Dacooder OptiLock FPS v6.1d | 4 / 4 | 298.7 | 161.3 | 296.9–301.4 |

These are medians of independent, nominal 26-second fight windows, excluding
loading, warm-up and resets. OptiLock's observed differences were +0.25% average
FPS and +1.21% 1% lows, with overlapping run ranges. No accepted nominal window
contained a frame above 16.7 ms. All seven completed passes reported zero tick
skew and the same 88 requested, 3 rejected and 59 observed casts, with 23 hits.

The phase mapping uses a server wall marker and the midpoint of the second in
MangoHud's filename, with -0.5/+0.5-second sensitivity analysis. It is not
validated render-clock alignment. The final series had one Sqooky client access
violation; an earlier OptiLock attempt crashed at the same `client.dll+0x17349c6`
offset. Cause remains unresolved. Failed attempts and excluded trigger pilots
are retained, and FPS summaries are conditional on completion. No validated
ranking or stability advantage follows from this small sample.

Version pins, settings hashes, attempt IDs and limitations are in
[comparison evidence](../tools/deadlock-benchmark/runtime/evidence/sqooky-optilock-main-comparison.json).
The complete local HTML/PNG report, raw streams and ledger are in the experiment
directory recorded there. The experiment's folder label is not its timestamp;
the ledger records the actual UTC run times.

## Playback timing follow-up, November 7, 2026

The persistent upstream checkout is `../modlock-investigation`, pinned to
v0.1.74. Its SDK exposed a scenario bug: `seek(0)` preserves the puppet clock,
and scoreboard time differs from the engine frame clock. The scenario now primes
paused puppets on the first frame and uses `frame.timeSeconds` throughout
playback. A probe using the real SDK reproduces the one-tick reset error and
passes with the correction, including different clock offsets and track endings.

Corrected live run `d9fff0da4e174584a9de21ca07ab10ca` completed warm-up and all
three measured passes with zero tick skew and maximum position residual about
0.000255 units. The earlier 1284.3-unit diagnostic matches a one-tick jump in
the recorded track; it is not evidence of that much physical displacement from
the correctly timed pose. This supersedes that interpretation in the older audit
below. An initial attempt using scoreboard time was rejected and retained as
run `0502bd1375054c5fa6d1551a4e9f06c0`.

Visible stutter remains unresolved. The corrected run reports a 130 ms slow
Modlock callback and 27 skipped updates, while captured client frame times after
the first 20 seconds remain below 19 ms. Upstream skips updates after callbacks
exceed its 8 ms allowance and aggregates warnings over up to one minute, so the
warning's location cannot precisely identify the slow callback's time. Planned
rewinds repeat about every 44 seconds (five-second lead-in, 30-second measurement,
about nine-second restore), not precisely every 30 seconds. Source telemetry now
records simulation and wall update gaps separately from reset transitions.
The Grimoire bundle has not been repackaged with this correction. No validated
score or performance improvement follows from the timing correction.

Instrumented follow-up `1435bb357c1b489cb76b6c9d4174f6af` completed all three
passes with zero tick skew and no delivered simulation-update gaps over 1.5
ticks or wall gaps over 50 ms during playback, including lead-in. Warm-up had
two simulation gaps (31.25 and 62.5 ms). Each pass repeats the same clip;
restoration visibly pauses and rewinds the fight rather than selecting a new
fight. This explains an intentional interruption, not every possible stutter.
Client frames after an arbitrary first 20 seconds peaked at 19.08 ms. Continuous
foreground focus was not established, and phase alignment remains unverified.
See [clock evidence](../tools/deadlock-benchmark/runtime/evidence/puppet-clock-investigation.json)
and [gap evidence](../tools/deadlock-benchmark/runtime/evidence/puppet-update-gaps.json).
The supervisor's final artifact registration hit a role-label conflict; capture
had flushed and all passes had ended. The ledger was finalized using original
artifact roles after verifying hashes, with the original exit error retained in
notes. No artifact contents or prior registrations were changed.

## Runtime setup audit, November 7, 2026

Follow-up execution: a separately installed, checksum-verified 0.1.74 CLI,
host and SDK completed the one-actor camera probe and all three full-fight
passes with hero precaching and UI/preview services disabled. The fixed view
intersected geometry. Full-fight actor drift and server stalls persist. Native
replay camera verification still failed, so the source-level observer enum
finding below must not be treated as a verified native console-control fix.
The default installation and Grimoire bundle remain unchanged. See
[execution evidence](../tools/deadlock-benchmark/runtime/evidence/setup-validation-v0174.json).

This follow-up checks the actual launcher, the SDK sources compiled into the
failed run, and upstream tags v0.1.17 and v0.1.74. It does not establish a crash
root cause or a performance improvement. No runtime upgrade was performed
during the source audit itself.

| Finding | Evidence and consequence | Next action |
| --- | --- | --- |
| Native observer-mode mistake | Both upstream tags define in-eye=2, chase=3, roaming=4 in `src/wasm/game_heroes.cc`. The diagnostic runner requested 5, then its correction requested 4. The screenshots showing Fly Cam are therefore not evidence that chase mode is broken. | Use mode 3 and verify the hero identity and camera visually before the observation starts. Do not assume controller slots and replay top-bar positions match. |
| Outdated spectator implementation | Installed CLI and host are v0.1.17. GitHub's release API reports v0.1.74, published November 7. The tag diff includes camera-pawn reuse, spectator play-state activation, safer hero removal/ID preservation, and persistent watch handling. | Test a separately pinned CLI/host/SDK release together, first with one actor and each camera mode. Keep the existing version as a baseline. These changes are relevant candidates, not proof that they fix our exit. |
| Missing precache step | The replay scenario never calls `precache`. The API already exists in v0.1.17 and requires calling during start before world load. Client logs contain blocking particle loads. | Resolve hero IDs to resource names and precache the fixture's heroes/resources during `onStart`; validate acceptance and remaining late loads. Warm-up does not replace this setup step. Logs do not prove that hero precaching alone removes the observed UI-particle loads. |
| CLI/UI launch mismatch | Grimoire passes `--ui-port 0`, but `play_lab.py` defaults to 43180. Recent direct diagnostic commands omitted this flag and enabled the interface service. Browser preview and watch were still disabled. | Use `--ui-port 0` for both entry points; include interface/preview/watch settings and full launch arguments in configuration provenance. Existing configuration hashes omit these settings, so matching them is insufficient. |
| Two motion mechanisms | `Puppet` places recorded poses each frame and teleports when the gap exceeds its default 200-unit threshold. The scenario also uses `drive` with synthetic attacks and movement-affecting abilities. This combination and the observed 1284.3-unit drift warrant investigation. | Compare motion-only and combat reconstruction, logging per-actor pose, puppet tick and correction events. Do not change the snap threshold to conceal drift. The v0.1.17-to-v0.1.74 puppet diff changes types, not this placement behavior. |
| Cold launch environment | Each session creates fresh server/client Proton prefixes and private server caches; the client launches directly through Proton rather than upstream's Steam launch. Video settings are copied, but Steam launch options/environment are not reproduced automatically. | Record cache policy and launch environment. Separate startup diagnostics from warmed measurement windows; verify relevant Steam/Proton differences before interpreting them as overhead. |

The basic upstream arena variables are already present: hibernation, solo bot
matches, King of the Hill, troopers, neutral spawns, practice bots and zipline
starts are disabled. We are not simply missing that preset. The boolean result
from a command is not a full runtime settings readback.

The generic launcher's default Midtown project is the older four-Wraith
`teamfight` example, while supervised replay completion expects `data/clip.json`
and `REPLAY PROBE` markers. Grimoire correctly supplies the replay scenario;
direct launches must do so explicitly. This also needs an early preflight
failure instead of discovering the missing clip after client launch.

Order of work: correct the native camera control and unify launch flags; add
precache/readiness/readback checks; validate an isolated current runtime with a
one-actor camera probe; then expand to twelve actors and motion/combat controls.
Do this before collecting five-run performance comparisons.

Primary sources:
[release 0.1.74](https://github.com/paralin/modlock/releases/tag/v0.1.74),
[spectator lifecycle](https://github.com/paralin/modlock/blob/v0.1.74/src/gameinterop/pawn_observer_players.cc),
[camera modes](https://github.com/paralin/modlock/blob/v0.1.74/src/wasm/game_heroes.cc),
[0.1.17 precache contract](https://github.com/paralin/modlock/blob/v0.1.17/src/wasm/wasm.proto),
[puppet motion](https://github.com/paralin/modlock/blob/v0.1.74/js/src/puppet.ts),
[arena preset](https://github.com/paralin/modlock/blob/v0.1.74/js/src/console.ts).

Reviewed November 7, 2026, US/Pacific (recorded at 2026-11-08 04:08 UTC).
Three GPT-6.1 Terra subagents reviewed public benchmarks, visualization and
aggregation independently. This document combines their findings with a review
of the current Grimoire implementation. It is a proposal, not shipped UI or a
claim that the existing captures are validated benchmark scores.

## Public references

| Reference | What exists | Practices relevant here |
| --- | --- | --- |
| [Deadlock Performance Lab methodology](https://github.com/itchyfeetleech/Deadlock-Performance-Lab/blob/main/docs/METHODOLOGY.md) | A replay/config comparison tool with Linux capture automation. | Fresh game launches, readiness acknowledgments, bounded measurement windows, configuration readback, retained raw logs and explicit rejection reasons. Its documentation acknowledges that command/logger timing is not tick-exact. |
| [CompLock benchmarks](https://deadlockconfig.com/docs/benchmarks/) | Published measurements of a 6v6 replay scene and individual configuration/mod changes. | Fixed chase camera, one seek per launch, excluded warm-up, interspersed baselines, reversed treatment order and reported drift/uncertainty. The results describe a particular PC, scene and game builds, not a universal score. |
| [CapFrameX](https://github.com/CXWorld/CapFrameX) | General Windows capture, analysis and comparison software. | Frame-time/FPS plots, distributions, hardware monitoring and comparison views. Useful visualization reference, not a Deadlock workload. |
| [PresentMon console](https://github.com/GameTechDev/PresentMon/blob/main/README-ConsoleApplication.md) | Windows frame-event capture. | Preserve PID, swapchain, timing definitions and frame type. Presented and displayed timing are different measurements. |
| [MangoHud](https://github.com/flightlessmango/MangoHud#fps-logging) | Linux instrumentation and logging. | Preserve raw source rows, recorder version and sensor availability. It does not supply a standardized Deadlock scene. |

These public methods were inspected, not independently reproduced. Their
published percentages, timing tolerances and confidence intervals are not
acceptance thresholds for our workload. Do not import their numbers into a
Grimoire leaderboard. Our percentile definition also differs from tools that
interpolate quantiles, so retain the metric algorithm and version in exports.

## What our evidence currently supports

Run `5bb4dcda75644533897896341bbd095c` contains 46,659 raw rows and 46,658
analyzed intervals over 184.33 seconds. Its 253.12 average FPS and 58.35 FPS
1% low describe the whole capture, including setup and warm-up. The largest
frame interval is 1306.25 ms. These are useful diagnostic facts, not isolated
teamfight scores.

Current limitations are substantive: pass boundaries are observed in server
logs with unverified client/capture clock mapping; effective display settings
are unknown; the actual selected GPU has not been verified from the inventory;
frame semantics and recorder overhead need validation; and the reconstruction
has cast and pose differences. Windows and Metal collectors remain stubs.

The present UI may expose diagnostics, configuration and provenance. It must
not imply that three sequential passes are three independent experiments or
that completed playback establishes comparison eligibility.

## Results UI

Use three related views: History, Run detail and Compare.

### History

Default to newest known recording date first. Keep unknown recording dates in
a clearly labelled group; show import date separately. Do not silently make an
old imported capture appear newly recorded.

| Default column | Meaning |
| --- | --- |
| Recorded | Local display time, with exact UTC and time-source details available. |
| Label / suite | Human description and experiment grouping. |
| Workload / scene | Native replay, Modlock reconstruction or synthetic test, with version. |
| Configuration | Short fingerprint and readable treatment label. |
| Playback | Completed, stopped, failed, interrupted or imported. |
| Capture | Collected, disabled, missing/corrupt or unavailable backend. |
| Measurement | Scope and validation state, independent of playback status. |
| Duration / frames | Coverage and sample size when known. |

Filters: recording/import date, suite, scene, label/run ID, configuration,
OS, GPU, game build, backend, playback outcome and measurement state. Failed
and incomplete runs stay discoverable. Optional diagnostic metric columns
must retain their scope labels; default sorting is never a fastest-run ranking.

### Run detail

Lead with the run label, configuration, date and a plain-language scope line:
"Whole capture, including loading, setup and warm-up. Clock alignment and frame
semantics are unverified."

Show metrics in a compact table with units, definitions, duration, sample count
and metric version. Provide raw data, logs, configuration diff, artifact checks
and dated notes. Render availability separately from numeric values; a
zero-only power column is not evidence of 0 W.

The primary chart is frame interval in milliseconds against elapsed capture
seconds. Provide zoom/reset, keyboard controls, exact-point inspection and an
accessible table/export. Preserve spikes using min/max envelopes for each
horizontal pixel bucket; add a typical trend only if clearly labelled.
Calculate statistics from all underlying intervals, never chart buckets.
Do not smooth by default or silently clip extreme values. A zoomed y-axis
must disclose values outside its range and offer the full extent.

Sensor charts use the capture time axis, separate scales, confirmed source
units, visible gaps and availability labels. Repeated sensor values are not
independent measurements. Do not infer a CPU/GPU bottleneck from utilization
alone. List server markers separately until their mapping to the plotted
capture clock is validated; no precisely shaded pass bands yet.

Grimoire already uses a bounded SVG approach in `MmrChart.tsx`, without a
general chart dependency. Reuse its interaction and theme conventions, but
not its smoothing or point-selection assumptions for frame-time spikes.
Choose bounded SVG or Canvas based on measured interaction cost. Parse and
aggregate CSV outside the renderer, request viewport-sized data over IPC,
and cache tiles by artifact digest, chart algorithm version and viewport.

Reuse the existing layout, form, table and status primitives, theme tokens and
localized strings. Color must not be the only signal for failures or series.

### Compare

Initially support side-by-side configuration and evidence comparisons. Show
every changed field and every missing control. Diagnostic traces can be
viewed separately in their own elapsed clocks; do not imply phase alignment.

After eligibility is established, show per-run dots, median/spread, explicit
baseline/candidate labels, absolute and relative changes, and exclusions.
Show small/noisy changes with their observed variation instead of automatically
calling them improvements. A multi-scene report stays broken out by scene
unless a versioned weighting policy was declared before the experiment.

## Comparison and aggregation contract

Retain the complete immutable configuration hash for provenance. Add a
separate, versioned experiment definition describing:

- The intentional treatment fields, such as a graphics option or mod set.
- Fixed controls, including scenario, camera, effective settings, selected GPU,
  game build, runtime, collector, frame semantics and metric definition.
- Workload and measurement-window policy, exclusions and aggregation rule.
- Baseline/candidate assignment, independent launch ID, block and run order.

Derive a controls fingerprint from known fixed fields only. Two unknown
values must not count as a verified match. Keep the complete config diff,
including differences outside the declared treatment. Cross-OS studies need
an explicitly validated protocol for backend/OS differences; different
hardware also prevents attributing a result solely to the OS.

Aggregate in this order: aligned pass window, named scene within a run, then
independent runs within a compatible experiment arm. Retain every underlying
value and excluded record. Report independent-run count and pass count
separately. For uncertainty, the resampling/experimental unit must reflect the
design, normally an independent launch or paired block, not each frame or
sequential pass. See [NIST experimental-design guidance](https://itl.nist.gov/div898/handbook/pri/section7/pri7.htm).

Start with individual values, median and min/max; add other summaries when
the sample and design support them. Do not average pass p99 values and label
the result a pooled p99. Either describe a median of pass metrics explicitly
or recompute a pooled statistic from the intended raw intervals, retaining
the distinctions and weighting policy.

For an eligible paired experiment, record `B - A` and `100 * (B / A - 1)` with
units and metric direction. Reject an undefined zero-baseline percentage.
Keep block-level differences visible. Use interleaved baselines or balanced
order to measure drift; do not claim statistical significance from three
correlated passes or borrow a fixed noise threshold from another machine.

## Query, sorting and schema evolution

The v1 ledger is an evidence store. Add query projections and versioned
analysis records rather than editing its original events or configurations.

| Addition | Required provenance |
| --- | --- |
| Analysis | Input artifact digests, analyzer/adapter/metric versions, creation time and protocol. |
| Window | Run/pass/scene identity, clock mapping and uncertainty, boundaries, coverage and warm-up flag. |
| Metric value | Scope, units, definition/version, value, sample count, duration and validity reasons. |
| Assessment | Evidence references, scope and policy version; keep integrity, capture, window and fidelity separate. |
| Experiment | Treatment fields, fixed controls, arm, block/order, independent launch and comparison policy. |
| Run summary projection | Recorded/imported dates, outcomes, backend, searchable metadata and scope-labelled diagnostics. |

Comparison eligibility is pair-specific. File integrity, successful capture,
valid timing and faithful workload are distinct checks, not one interchangeable
"passed" flag. Existing boolean assertions in the offline analyzer are not a
substitute for evidence-backed assessments.

Use an allowlist of sort fields and keyset pagination. Proposed defaults:

- Recorded time descending, unknown dates last, then stable run ID.
- Explicit `time_basis=recorded|created`; UTC half-open date ranges.
- Local calendar filtering converts both boundaries with their timezone/DST
  offsets. Retain UTC, clock source and original offset where known.
- Numeric missing values last in both sort directions; never substitute zero.
- Metric sorting requires a common definition, units and scope. An explicit
  diagnostic sort may help investigate hitches, but must not imply benchmark
  ranking or calculate performance deltas. Eligible comparisons additionally
  require the experiment's control and validation checks.

Current implementation gaps: `list` filters a recording/import-date fallback
but sorts creation date; it reconstructs a full export for every summary row;
there is no paginated history, pass-window table, analysis-to-ledger link,
experiment assignment or evidenced cross-backend equivalence. The analyzer
requires distinct file hashes, but distinct files do not prove independent
experiments.

Candidate indexes: recording date plus run ID; suite plus recording date;
configuration hash plus recording date; run/pass/scene for windows; metric
definition/scope/value for supported metric sorts. Confirm useful indexes
with actual queries and `EXPLAIN QUERY PLAN`, following
[SQLite's query-planner documentation](https://www.sqlite.org/queryplanner.html).

Exports include filters, sort order, snapshot cutoff, schema/analysis/metric
versions, run IDs, exclusions, units, window policy, configuration differences
and artifact hashes. A shareable export should omit absolute local paths by
default; the local evidence export can retain them for diagnosis.

## Implementation order and acceptance

1. Add typed read-only IPC for paginated history/detail and configuration diff.
   Test unknown import dates, missing data, stable ties and failure visibility.
2. Add Run detail, diagnostic metrics, evidence links and notes. Keep scope and
   unverified fields visible. Do not change current eligibility flags.
3. Add frame-time and sensor views with worker-side aggregation. Verify spikes
   survive downsampling and statistics do not depend on zoom or chart width.
4. Add versioned windows and assessments; validate client/capture clock mapping,
   effective settings, selected renderer/GPU and recorder overhead.
5. Add experiment controls, independent launch/block scheduling and eligible
   comparisons. Evaluate native replay as a reference workload separately from
   the current Modlock reconstruction. Validate each OS backend on real hardware.

The first three steps can deliver useful history and diagnostics immediately.
The final two establish whether performance comparisons are defensible.
