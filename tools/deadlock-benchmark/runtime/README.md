# Runtime compatibility probe


## Automatic capture and ledger

The supervised launcher now supports `--capture auto` (or an explicit
`mangohud`) and `--suite` / `--label`. See [capture and ledger](CAPTURE_LEDGER.md)
for supported backends, raw statistics, date/configuration queries, per-pass
notes and historical imports. Capture-wide diagnostics remain separate from
validated measured-pass scores.

## Try the Midtown teamfight probe

From the repository root, start the regular Midtown map and scripted teamfight:

```sh
python3 tools/deadlock-benchmark/runtime/play_lab.py restart --map midtown --demo
```

With `--demo`, the probe starts once the player joins; place the player in a
clear area first when starting manually. Omit `--demo` and type `/bench_start`
in chat for manual control. The probe sets up bots, runs a warm-up and a
measured interval, and displays markers.
Use `/bench_stop` to stop the probe and restore the view. It is an experimental
local-server workload; camera, effects and capture alignment need verification
before treating a run as a benchmark result. See the teamfight scenario for
its exact behavior and limits. The launcher also supports `status` and `stop`.

## Try the lab

From the repository root, run one of these commands on this machine:

```sh
python3 tools/deadlock-benchmark/runtime/play_lab.py restart --map 16  # effects
python3 tools/deadlock-benchmark/runtime/play_lab.py restart --map 0   # baseline
python3 tools/deadlock-benchmark/runtime/play_lab.py status
python3 tools/deadlock-benchmark/runtime/play_lab.py stop
```

The launcher uses the local Modlock host, Proton and the compiled CSDK13 lab
maps. `--help` lists path and port overrides. It starts a local server and a
windowed client with the Modlock interface enabled. Once connected, open chat
and type `/bench_start`: the scenario requests a fixed camera and displays a
30-second warm-up followed by 60 seconds between start and end markers. Avoid
movement during the run. The camera, markers, timing alignment and capture
remain experimental; do not treat this as a validated benchmark score.

The game runs from a writable isolated copy at
`/tmp/deadlock-benchmark-runtime-probe`, never from the installed game. If the
copy does not exist, the launcher creates it using `rsync`, omitting retail
addons, replays and Modlock state; game VPK assets are linked for reading. The
first copy needs about 1 GB and additional space for isolated Proton prefixes.
If the installed game updates, use `--game` to select a new, non-existent
isolated path so it can be copied again. If the compiled maps are elsewhere,
pass their directory with `--maps`. This launcher does not download or update
anything. Session state and logs live under `~/.cache/deadlock-benchmark-play`;
`stop` and `restart` address only the saved session and its unique prefixes.
The UI service is local; Modlock uses its configured HTTPS relay to display
the overlay. The server uses the configured UDP port, which may bind beyond
loopback.

## CSDK13 compile and current-game load

The former mixed CSDK stage fails registration; a separately extracted CSDK13
cohort compiles both generated maps. The package's bundled game version is
6836 / revision 11193655; the installed runtime used for the smoke test was
6860 / 11211860. These are separate version facts, not a claim that the older
compiler is a current retail binary. The [package provenance](evidence/csdk13-provenance.json)
records the community download and archive SHA-256
`32c64c4e3a6e3f17ed92e89e8941e2f0034696900d762e1370f682780f888386`.
Its bundled description identifies its `game/bin` tools as a Dota 2 cohort
from October 7. Do not redistribute its bundled game files.

Extract the package into a **dedicated writable directory**, outside the
installed game and any shared SDK. The preparation command keeps its binaries
intact, copies the package's `readonly_tools_asset_info.binOFF` to the enabled
name, and links missing `pak01_*.vpk` and `shaders_*.vpk` from the local retail
game into the stage's real `game/citadel` and `game/core` directories. Links
must be treated as read-only assets; they follow retail updates. Existing
package files are kept; conflicting links or enabled asset info are refused.
It does not copy or update the retail game. For this machine:

```sh
python3 tools/deadlock-benchmark/runtime/prepare_csdk13.py \
  --stage /mnt/storage/csdk13-benchmark/Reduced_CSDK_13 \
  --retail-game /mnt/storage/SteamLibrary/steamapps/common/Deadlock/game \
  --acknowledge-isolated-stage
python3 tools/deadlock-benchmark/map/generate.py \
  --template /mnt/storage/csdk-native/content/citadel_addons/particle_bench/template/volumetric_test.vmap \
  --output /tmp/deadlock-lab-source-new
python3 tools/deadlock-benchmark/runtime/compile_csdk13.py \
  --stage /mnt/storage/csdk13-benchmark/Reduced_CSDK_13 \
  --sources /tmp/deadlock-lab-source-new \
  --proton /home/esoc/.cache/modlock/escort-proton/proton \
  --steam /home/esoc/.local/share/Steam \
  --results /tmp/deadlock-lab-compile-new \
  --archive-sha256 32c64c4e3a6e3f17ed92e89e8941e2f0034696900d762e1370f682780f888386 \
  --acknowledge-isolated-stage
```

The source and results paths must be new. Each compile uses a new addon and a
separate Proton prefix, saves logs and hashes in a manifest, and requires a
zero exit status, a successful nonempty compiler summary and a fresh VPK.
Timeout kills its own process group and asks only the dedicated prefix's
wineserver to stop; a failed cleanup is recorded. Inspect the logs and run the
map verifier separately. The compilation uses `-nolightmaps`; it does not
suppress schema mismatches. Both maps compiled with 9 compiled, 0 failed; both
logs also reported missing editor materials `handle_generic_aligned.vmat_c`
and `handle_generic_aligned_translucent.vmat_c`. Their rendering impact is
unknown. See the [wrapper manifest](evidence/csdk13-wrapper-manifest.json) and
[zero](evidence/csdk13-no-override-lab0.log) and
[16-emitter](evidence/csdk13-no-override-lab16.log) logs. Output hashes include
the unique addon path and can differ across builds.

Both no-override VPKs were independently checked for world, physics, entity
and node data; the 16-emitter map contained the expected named active emitters.
Both loaded on a build 6860 server, and a headless client joined the 16-emitter
map and created a DXVK swapchain. See the [zero-map server log](evidence/csdk13-lab0-server.log),
[16-map server log](evidence/csdk13-lab16-server.log), and
[client render excerpt](evidence/csdk13-lab16-render.log). The exact VPK hashes
are in the wrapper manifest. This does not prove visible effects, camera
position, benchmark timing or capture alignment. Verify those in a visible
client before using results.

## Historical preflight and runtime checks

`preflight.py` inventories versions without changing the game or SDK. For this
machine:

```sh
python3 tools/deadlock-benchmark/runtime/preflight.py \
  --game /mnt/storage/SteamLibrary/steamapps/common/Deadlock \
  --stage /mnt/storage/csdk-native \
  --map /mnt/storage/csdk-native/game/citadel_addons/particle_bench/maps/particle_bench_16.vpk
```

The default `retail-mixed` profile exits 1 if build/revision is missing or
mismatched, or the compiler executable is missing. A matching stage is not a
compatibility certification.

For the separately downloaded CSDK 13 package, use:

```sh
python3 tools/deadlock-benchmark/runtime/preflight.py \
  --game /mnt/storage/SteamLibrary/steamapps/common/Deadlock \
  --stage /mnt/storage/csdk13-benchmark/Reduced_CSDK_13 --profile csdk13
```

This profile inventories a package marker and selected compiler dependencies,
records their SHA-256 hashes, and exits 1 if any are missing or unreadable.
Exit 0 means this limited inventory is complete; hashes are for provenance and
are not checked against an authenticated release manifest. It does not prove
the package is coherent, can compile, or produces output compatible with the
current game. Retail and SDK build/revision and their equality are reported
independently: a difference is expected for this package, and neither a match
nor an inventory success establishes runtime or compiler-output compatibility.
The tool makes no downloads or updates.

## 2026-11-05 host check

The installed game reports build 6860 / revision 11211860. Both existing SDK
stages report 6819 / 11162696, and their binary links point into the updated
retail game. No current matching staged toolchain was established. MangoHud
0.8.4 and Modlock 0.1.17 are installed; presence alone does not validate capture.

A bounded, server-only test copied the non-VPK files from retail into
`/tmp/deadlock-benchmark-runtime-probe/game`, excluding retail addons, replays
and modlock state; VPKs were linked read-only in intent to retail assets. It
copied the benchmark scenario and installed Modlock type declarations to a
separate project directory. It used the cached v0.1.17 host and Proton, a new
XDG cache/config and Proton prefix, disabled update checks, and ran:

```sh
DEADLOCK_DIR=/tmp/deadlock-benchmark-runtime-probe \
MODLOCK_HOST=/home/esoc/.cache/modlock/host/v0.1.17/modlock-host.exe \
MODLOCK_PROTON=/home/esoc/.cache/modlock/escort-proton \
MODLOCK_NO_UPDATE_CHECK=1 \
XDG_CACHE_HOME=/tmp/deadlock-benchmark-runtime-probe/cache \
XDG_CONFIG_HOME=/tmp/deadlock-benchmark-runtime-probe/config \
timeout --signal=TERM --kill-after=10s 25s \
  modlock dev --no-game --port 27183 --preview-port 0 --ui-port 0 \
  /tmp/deadlock-benchmark-runtime-probe/scenario
```

[The log](evidence/host-probe.log) reports the scenario started and the stock
`dl_midtown` server became ready. Its server log also reports player commands
ready and game state `GameInProgress`. Timeout ended the test. No player, lab
map, camera, effect, visible marker, renderer or frame capture was tested.
The tested `main.tsx` SHA-256 was
`1445fa421748722a6c4f9d66bbb9efc1e1974adbbb08aef76afc2b4449158345`.
The log is evidence for this particular build and host, not a general support
claim.

A separate copy of the old CSDK stage was used for a compile attempt of the
new `dl_benchmark_lab_0.vmap`, with a fresh Proton prefix. The copy retained
its links to retail DLLs and assets and was therefore a mixed, stale toolchain.
Resourcecompiler was invoked from its copied `game/bin/win64` directory with `-game
citadel -addon benchmark_probe -fshallow -nop4 -v -consoleapp -consolelog
-condebug -toconsole -danger_mode_ignore_schema_mismatches -world -phys
-nolightmaps -threads 8 -filelist <isolated filelist>`. It stopped before a
compile summary with `RegisterConVar: Unknown error registering convar
"mesh_calculate_curvature_smooth_pass_count"`; see [compiler log](evidence/compiler-probe.log).
No compiled lab map was produced. This is an observed failure, not a diagnosis
of its cause. Do not use this old stage to claim a current map compile.

These were the limitations at the time of this old-stage experiment. The
CSDK13 results above supersede its compiler and server-load findings; visible
effects and marker alignment still require a client-side check.

A separate test copied the historical `particle_bench_0.vpk` from the SDK
stage into the isolated game's `citadel/maps` and added `"map":
"particle_bench_0"` to a temporary copy of the scenario manifest. The VPK
SHA-256 is `4a0f3eec3f1167476f784a2be046fc38d84cfc4a58628218e89a0a782bce163e`.
The [host log](evidence/historical-map-probe.log) reports the server ready on
that map, and its server log reaches `GameInProgress`. This is the historical
map, not the generated `dl_benchmark_lab_0`; it establishes server-side map
loading only. No player or client rendering was checked.

The same isolated test selected historical `particle_bench_16.vpk` (SHA-256
`6ad612f27ee13fdf4fff601e3d2cc2fa38bd999ed14907e788becfafd7990583`).
The [server log](evidence/historical-map-16-probe.log) reports that map ready.
A direct client in a separate Proton prefix, run under headless Gamescope,
connected to the server. Its [render log excerpt](evidence/client-render-probe.log)
reports a DXVK swapchain. This does not establish that particles were visible,
that the view was correct, or that any per-frame capture worked. No screenshot was obtained. Both historical maps remain different from the
new lab maps.

A second headless client run enabled MangoHud 0.8.4 with `autostart_log=1`,
`log_duration=20`, `log_interval=0`, `permit_upload=0`, and an isolated output
folder. The unmodified [raw CSV](evidence/mangohud-client-probe.csv) contains
7,316 rows after its metadata and header. SHA-256:
`a5a4c546ce7aec03392186abced85e13216fc28a0d3a920159641c59f27c0cb2`.
For all 7,315 consecutive row pairs, the difference in `elapsed` divided by
1,000,000 closely matches the later row's `frametime` in milliseconds: median
absolute difference 0.000108 ms, 99th percentile 0.002199 ms, maximum 0.01656
ms. This supports that these are individual frame records in this test, but
the precise presented/displayed-frame semantics are still unverified. The
file includes startup and has no scenario markers or verified view; it is an
invalid benchmark run and must not be scored. The installed MangoHud example
describes these logging options, with `log_interval=0` the default.

`mangohud_to_frames.py SOURCE NEW_OUTPUT.csv` converts this observed 0.8.4
layout to the analysis tool's `per-frame-csv` columns, preserving every row
and printing the source SHA-256 and count. It rejects non-increasing or
inconsistent intervals and refuses to overwrite an output. Conversion does not
validate capture boundaries or frame semantics. Record the printed provenance
and keep the raw CSV. The retained smoke capture is not a valid benchmark.

## 2026-11-05 compiler investigation

The historical `rc.sh` exists in another local checkout, and its exact working
directory and arguments reproduced the same failure. The copied stage lacked
`tshim.dll` and its ModelDoc library was the unshimmed donor; overlaying the
historical shim build into a second isolated copy still resulted in the same
[registration failure](evidence/compiler-shim-probe.log). Temporarily removing
`smartprops.dll` from the first isolated copy failed earlier with an explicit
[required dependency error](evidence/compiler-no-smartprops.log); it was restored.
As a diagnostic only, changing one convar name in a copied ModelDoc library
advanced the fatal to the next curvature convar, implicating that library's
registration path. The [diagnostic log](evidence/compiler-modeldoc-name-diagnostic.log)
is not a usable compiler result; the library was restored byte-for-byte and no
map was produced from the modified binary. These experiments establish neither
the root cause nor a safe workaround.
