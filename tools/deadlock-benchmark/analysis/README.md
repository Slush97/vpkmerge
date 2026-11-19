# Capture analysis (experimental)

The launcher can now collect raw Linux frame data and maintain a local run
ledger; see [capture and ledger](../runtime/CAPTURE_LEDGER.md). Its whole-capture
diagnostics do not satisfy this analyzer's explicit alignment requirements.

This offline Python 3 standard-library tool analyzes one explicitly aligned frame stream. It does not control the game, discover markers, validate effects, or prove that a capture tool observed Deadlock. No benchmark score or real capture is supplied. Verify those things independently before setting the checks to `true`.

## Capture and alignment

Capture the entire warm-up and measured route, with extra time on both sides. Identify the visible start and end markers in the capture clock, for example with a separately recorded synchronized video, and document the synchronization method and evidence. Record the corresponding scenario clock values separately; do not infer capture times from scenario durations. Check for pauses, missing effects, loading, data loss, and unsupported builds. Preserve the unmodified raw file and alignment evidence. The tool requires an event at or before the start and at or after the end; it selects events with timestamps **greater than start and less than or equal to end**. Each selected event's full preceding frame interval is included even if it crosses the start marker. Average FPS is the selected event count divided by the marker-to-marker elapsed capture time. The boundary convention can affect a result by about one frame and should be kept identical for comparisons.

`presentmon` reads raw PresentMon CSV columns `TimeInSeconds` and `MsBetweenPresents`, interpreted as the time of the Present call and the elapsed time since the previous Present, respectively ([PresentMon capture documentation](https://github.com/GameTechDev/PresentMon/blob/main/README-CaptureApplication.md)). It measures **presented**, not displayed, frames. Export one target process and one swapchain/frame stream; verify this yourself. Aggregated statistics, interleaved streams, and display timing are not supported. Explicitly record tool version and frame-generation behavior. If generated and application frames cannot be distinguished, record that limitation in metadata and do not claim they were distinguished.

`per-frame-csv` reads `timestamp_seconds,frame_time_ms`, one row per frame, with a timestamp in the same capture clock as the markers and a positive duration ending at that timestamp. Use only after verifying the exporter and the stream's actual semantics; identify the source, conversion method, and whether timings are presented or displayed in metadata. Keep the original export as well as any converted CSV. MangoHud logging is not assumed to have per-frame semantics: verify the actual exporter and renderer first. Both adapters require finite values and strictly increasing timestamps, including rows outside the interval. Consecutive timestamps must agree with each frame interval within 1 ms, catching some missing or interleaved rows; this does not prove there was no capture loss. PresentMon permits a zero duration only in the first row, treated as an undefined boundary event that cannot be selected. Do not drop rows to improve results.

## Manifest

Save a JSON file next to the capture. Paths are relative to the manifest. This is a schema example, **not** an actual run; replace every value with observed evidence. Metadata may contain extra keys and should include all relevant settings (resolution, window mode, render scale, upscaler, preset/overrides, cap, VSync, refresh), hardware (CPU, GPU, memory), and capture method. The complete metadata object must match for repeated runs; use the same structure and values. `frame_generation` must describe the mode and any limitations in separating generated frames. Use an empty list for `mods` when no mods are present. Missing or unknown required top-level metadata makes the run invalid. The tool does not validate individual settings or hardware fields inside those objects; the operator must record and verify every item.

```json
{
  "adapter": "presentmon",
  "raw_capture": "capture.csv",
  "markers": {
    "capture_start_seconds": 10.0,
    "capture_end_seconds": 70.0,
    "scenario_start_seconds": 20.0,
    "scenario_end_seconds": 80.0,
    "alignment_method": "describe clock synchronization and evidence file"
  },
  "checks": {
    "warmup_completed": true,
    "supported_build": true,
    "complete_interval": true,
    "effects_verified": true,
    "no_load_screens": true,
    "no_unexpected_pause": true,
    "no_lost_capture_data": true,
    "target_process_verified": true
  },
  "invalid_reasons": [],
  "metadata": {
    "planned_duration_seconds": 60,
    "benchmark_version": "observed version",
    "map_hash": "observed content hash",
    "scenario_version": "observed version",
    "scenario_hash": "observed content hash",
    "game_build": "observed build",
    "capture_tool": "PresentMon",
    "capture_tool_version": "observed version",
    "frame_semantics": "presented",
    "frame_generation": "observed mode and separation limitations",
    "server_setup": "observed setup",
    "settings": {"resolution": "observed", "window_mode": "observed", "render_scale": "observed", "upscaling": "observed", "preset_and_overrides": "observed", "fps_cap": "observed", "vsync": "observed", "refresh": "observed"},
    "hardware": {"cpu": "observed", "gpu": "observed", "memory": "observed"},
    "os": "observed",
    "gpu_driver": "observed",
    "mods": "none or list and versions",
    "shader_cache_policy": "observed policy"
  }
}
```

Set checks to `true` only with evidence; false or missing checks invalidate the run. Add any other known failures as strings in `invalid_reasons`. A failed run produces `run.json` with reasons and no metrics or graph. The raw path and SHA-256 are recorded when the capture is readable. Keep it and the manifest with the results; the program does not copy, modify, or remove captures. Hashes provide identification, not authenticity.

```sh
python3 tools/deadlock-benchmark/analysis/benchmark.py run run1/manifest.json --output results/run1
python3 tools/deadlock-benchmark/analysis/benchmark.py report results/run1/run.json results/run2/run.json results/run3/run.json --output results/report.json
python3 -m unittest discover -s tools/deadlock-benchmark/tests
```

Output paths must be new. Run exits 0 for valid and 2 for invalid/error. Each valid run has an SVG of all selected frame times and JSON metrics. The 1% low is `1000 / mean(slowest ceil(frame_count * 0.01) frame times in ms)`; p99 is the nearest-rank frame time at rank `ceil(0.99 * frame_count)` in ascending order. Long frames are retained. Metric version 1.1 also reports nearest-rank p95 frame time. The report shows each run, median, minimum and maximum without pooling frames. It requires at least three valid, distinct captures and identical metadata and versions, including a positive planned duration. Actual capture and scenario marker durations are saved per run and may differ. Review these durations against your documented experiment-specific drift policy before considering runs comparable; there is no universal automatic drift threshold. Identical raw files are rejected even if distinct windows were intended; export independent captures for this report. Compare A/B reports separately and alternate trial order; no statistical significance or automatic pass/fail threshold is implied. Investigate variation before drawing conclusions.

## Local community-config investigation

The `prepare-config-comparison.ts`, `run-config-trial.py`,
`run-config-comparison.py` and `report-config-comparison.py` tools support the
local Sqooky Default / OptiLock FPS investigation. These are experimental
machine-specific harnesses, not an alternative way to satisfy the validated
analyzer checks above. They retain `measurement_valid=false` and
`comparison_eligible=false`.

The prepared experiment directory contains pinned upstream text files and
hashes in `upstream-inputs.json`, an isolated `game/`, a common `baseline-cfg/`,
a single-repetition replay `scenario/`, generated `variants/`, and a
`comparison-plan.json` with the fixed attempt order and harness hash. The preset
preparer calls Grimoire's actual generation and patching code with Steam userdata
discovery disabled; all outputs are private files. It refuses existing variant
outputs. Display resolution, render scale, upscaling, cap and aspect controls
are shared, while other author visual settings differ. Optional VPK bundles
are excluded. The selected Sqooky preset has no sibling video file, so its other
video settings inherit the snapshotted baseline.

```sh
python3 tools/deadlock-benchmark/analysis/run-config-comparison.py /path/to/prepared-experiment
python3 tools/deadlock-benchmark/analysis/report-config-comparison.py /path/to/prepared-experiment
```

Each attempt gets a fresh private cache and Proton prefixes, restores the common
cfg snapshot, warms the full clip once, and records continuously. Analysis takes
a nominal 26-second window, two seconds inside each measurement boundary.
The estimate relates the scenario's wall-clock marker to the middle of the
second in MangoHud's filename. MangoHud 0.8.4 source resets elapsed time and
names the file when logging starts. Results also show offsets of -0.5 and +0.5
seconds. This sensitivity check is neither validated render-clock alignment nor
a guaranteed timing error bound. Whole-capture diagnostics remain separately
labeled; only the guarded-window metrics enter the comparison.

The runner resumes a matching partial attempt list, retains failed attempts,
and never substitutes an FPS score for a crash or failed protocol check. The
report verifies registered artifact hashes, checks successful pass markers and
saved display settings, and reports completion counts, per-run metrics,
medians, ranges, timing sensitivity and an HTML/PNG report. Foreground checks
are sampled once per second. Failed launches and any protocol amendments remain
visible in the plan, ledger and reports. This investigation has observed
client access violations with both presets; successful-run FPS alone cannot
establish stability or predict ordinary match performance.

The retained Sqooky Default / Dacooder OptiLock FPS pilot also has two optional
offline renderers. They read `comparison-results.json` and its referenced run
artifacts, then write to the experiment's `graphs/` directory:

```sh
python3 tools/deadlock-benchmark/analysis/render-config-graphs.py /path/to/prepared-experiment
python3 tools/deadlock-benchmark/analysis/render-config-interactive.py /path/to/prepared-experiment
```

The static renderer requires NumPy and Matplotlib; the interactive renderer
requires Plotly. These renderers retain the pilot's preset names and narrative
annotations (including its completion counts); review those annotations before
using them with another experiment. They are exploratory reports, not a
general-purpose benchmark dashboard.
