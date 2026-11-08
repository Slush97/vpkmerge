# Capture and run ledger

The launcher collects local evidence and writes a dated history. Playback
completion, raw capture collection, measurement validity and comparison
eligibility are separate facts. The current reconstruction remains unscored.

## What is implemented

| Backend | State |
| --- | --- |
| Linux / MangoHud 0.8.4 | Automatic raw capture, normalization, sensor rows and diagnostic statistics. |
| Windows / PresentMon | Adapter contract only. Capture is not implemented or tested here. |
| macOS / Metal HUD | Adapter contract only. Capture and Mac benchmark hosting still need implementation and testing. |

The experimental Grimoire launcher requests `--capture auto`. A supported
MangoHud installation starts recording with the game. An unavailable recorder
is recorded as disabled with a reason; it never silently produces FPS results.
An explicitly requested unsupported backend fails before launching the game.
No collector is downloaded, and captures are not uploaded.

For a direct local run:

```sh
python3 tools/deadlock-benchmark/runtime/play_lab.py start \
  --game /path/to/isolated-game --retail /path/to/Deadlock \
  --scenario /path/to/scenario --state-dir /path/to/benchmark-history \
  --demo --supervise --capture mangohud --resolution 1920x1080 \
  --suite nightshift-baseline --label '1080p baseline, run 1'
```

Use `--capture none` to disable recording. Automatic capture requires
`--supervise`, which owns process shutdown and finalizes the evidence after
the client exits. Stop and failure paths also retain available evidence.

## Files and measurement scope

The state directory contains `ledger.sqlite3` and one directory per session.
Each recorded session has:

| File | Contents |
| --- | --- |
| `configuration.json` | Immutable startup configuration, scenario/map/runtime hashes, hardware facts and known unknowns. |
| `display.json`, `video-settings.txt` | Requested resolution and the video configuration written to the isolated game. Effective renderer settings still need independent verification. |
| `server-launch.log`, `client.log` | Original diagnostic logs. |
| `capture/plan.json`, `capture/MangoHud.conf` | Recorder version and exact logging settings. |
| `capture/deadlock*.csv` | Unmodified source frame and sensor data. |
| `capture/frames.csv` | Normalized `timestamp_seconds,frame_time_ms` records. |
| `capture/telemetry.csv` | Sensor values with the source elapsed timestamps and column names. |
| `capture/result.json` | Row validation, sample count, diagnostic metrics, sensor ranges and limitations. |
| `result.json` | Playback outcome, run ID and capture result. |
| `run-ledger.json` | Portable export of this run's configuration, event history and artifact hashes. |

The first statistics describe the **whole capture**, including loading, setup
and warm-up. They are useful for checking that measurement works. They are not
three measured-pass scores and must not be presented as such. The first event
is used as a time boundary; subsequent events supply the intervals and frame
count. No long frames are discarded.

Both the validated-window analyzer and capture diagnostics use
`benchmark_metrics.py` (metric version 1.1):

- Average FPS: selected frame count divided by elapsed seconds.
- 1% low: 1000 divided by the mean of the slowest `ceil(N * 0.01)` intervals.
- p95/p99: nearest-rank frame-time percentiles, in milliseconds.
- Maximum frame interval, frame count and duration.

MangoHud sensor values can repeat between frames. Minimum/maximum values are
reported without treating each repeated row as an independent sensor sample.
All-zero fields are labelled `zero_only_or_unavailable`, not evidence of zero
power, temperature or utilization. Preserve the source column units; do not
combine unlike Windows, Linux and Metal sensor definitions.

## History granularity

| Level | Recorded identity and purpose |
| --- | --- |
| Suite | A chosen label grouping related experiments, such as `nightshift-baseline`. |
| Configuration | SHA-256 of canonical configuration JSON. Key ordering does not change the identity. |
| Run | Unique ID, UTC creation time, known original recording time, label, origin and directory. |
| Pass | `pass_index` on markers and notes. Zero means warm-up; positive values are measured passes. |
| Event | Ordered sequence, UTC observation time, monotonic clock value for live observations, kind and payload. |
| Artifact | Relative path, size, SHA-256, role and registration time. |
| Frame/sensor row | Kept in CSV files, not duplicated into the ledger. |

Configuration hashes deliberately include the game, scenario, runtime and
recorder versions. They exclude the run date and label. A matching hash is
useful for grouping, but **does not establish comparison eligibility**. Unknown
render settings, different hardware stacks, capture overhead, alignment and
scenario differences need separate validation.

The ledger is append-only through its API. Notes correct or qualify prior
observations without editing them. A configuration change belongs to a new
run; `diff` shows exactly which recorded fields changed. Hashes detect accidental
changes to saved evidence; the database is not a signed or tamper-proof audit
system. Keep it on local storage and retain the referenced session directories.

Dates are stored in UTC with microsecond formatting. Formatting precision does
not imply measurement accuracy. Date-only query arguments mean midnight UTC;
explicit times require an offset. Range ends are exclusive. Use calendar
windows to retrieve an hour, day, week or month, and suite/config filters for
experiment granularity. Imported runs with unknown recording dates use their
import date for filtering and retain `recorded_utc: null`.

## Read, document and update a run

Use the ledger in your benchmark state directory. Grimoire stores it under
its `userData/benchmark/` directory.

```sh
# All runs in a UTC day, including failures and cancellations.
python3 tools/deadlock-benchmark/runtime/benchmark_ledger.py --db /path/to/ledger.sqlite3 \
  list --since 2026-11-08 --until 2026-11-09

# Filter an experiment. Add --config FULL_CONFIG_HASH to require an exact snapshot.
python3 tools/deadlock-benchmark/runtime/benchmark_ledger.py --db /path/to/ledger.sqlite3 \
  list --suite nightshift-baseline

# Append a dated observation about a particular pass. Omit --pass for a run note.
python3 tools/deadlock-benchmark/runtime/benchmark_ledger.py --db /path/to/ledger.sqlite3 \
  note RUN_ID 'Visible hitch near the second ability sequence' --pass 2

# Compare recorded configurations; this does not compare or approve FPS scores.
python3 tools/deadlock-benchmark/runtime/benchmark_ledger.py --db /path/to/ledger.sqlite3 \
  diff BASELINE_RUN_ID CANDIDATE_RUN_ID

# Export a fresh snapshot after adding notes. Existing export files are not replaced.
python3 tools/deadlock-benchmark/runtime/benchmark_ledger.py --db /path/to/ledger.sqlite3 \
  export RUN_ID --output run-with-notes.json

# Verify that saved evidence still matches its registered hashes.
python3 tools/deadlock-benchmark/runtime/benchmark_ledger.py --db /path/to/ledger.sqlite3 \
  verify RUN_ID

# Register older sessions without pretending they had automatic capture.
python3 tools/deadlock-benchmark/runtime/benchmark_ledger.py --db /path/to/ledger.sqlite3 \
  import-session /path/to/old-session --suite historical --label 'Earlier playback check'
```

For an import, provide `--recorded-at 2026-11-07T19:09:24-08:00` only if that
original time is known. Otherwise leave it unknown. Historical hardware is
never filled in from the machine doing the import. Importing the same source
directory twice is refused. The source files are not modified.

After a normal end, the run export is refreshed automatically. Notes added
later are immediately in SQLite; use `export` for a new portable snapshot.
After a hard interruption, unfinished runs remain visible. The launcher's
external Stop path records interrupted cleanup without claiming complete data.

## Timing and next validation gates

Current scenario markers preserve the scenario's game clock and the
supervisor's log-receipt clocks. The supervisor polls at 250 ms; that interval
is **not** a proven error bound. Log buffering, transport and client rendering
can add delay. `alignment_verified` remains false and uncertainty is unknown.
Do not use receipt time or CSV file creation time as an exact frame boundary.

Before automatic measured-pass comparisons, implement and validate client
markers mapped into each collector's clock, establish frame semantics and
recorder overhead, verify settings at runtime, and bound scenario variation.
Missing data or insufficient evidence must retain the raw capture with a clear
failure reason. The current collector never sets measurement validity or
comparison eligibility to true. The existing explicit-alignment analyzer
remains a separate step.

Source contracts: [PresentMon](https://github.com/GameTechDev/PresentMon/blob/main/README-ConsoleApplication.md),
[MangoHud](https://github.com/flightlessmango/MangoHud/blob/master/data/MangoHud.conf),
and [Apple's Metal HUD logging](https://developer.apple.com/documentation/xcode/monitoring-your-metal-apps-graphics-performance).
