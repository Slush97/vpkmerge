# vpkmerge

Combine multiple Valve Pak (`.vpk`) files into one. Built for **Deadlock** modding: the engine caps mounted mod VPKs at roughly 100, so pre-merging lets players run more mods than the engine otherwise allows.

Published at https://github.com/Slush97/vpkmerge (MIT).

## Layout

Rust Cargo workspace:

```
vpkmerge-core/        pure-Rust merge engine (lib, v0.3.0)
  src/lib.rs          public API: inspect / detect_conflicts / merge / split
vpkmerge-cli/         CLI binary `vpkmerge` on top of core (v0.2.0)
gui/
  src/                Vue 3 + Vite + Tailwind 4 frontend
  src-tauri/          Tauri v2 desktop app wrapping the same engine
morphic/              pure-Rust Source 2 .vtex_c decoder + KV3 codec (lib, v0.0.1)
  src/                resource / kv3 / texture modules
  src/kv3/            binary KeyValues3 codec (reader v1..=5 + LZ4, writer v4 uncompressed)
  fixtures/           committed canonical corpus (.vtex_c + .png + .meta.json; kv3/ holds .vsndevts_c)
  tests/golden.rs     diffs morphic's decode against oracle PNGs
  tests/kv3.rs        decode + uncompressed-v4 round-trip against gigawatt.vsndevts_c

tools/morphic-oracle/   dev-time C# harness that generates the goldens
  Program.cs            wraps ValveResourceFormat; subcommands: generate/extract/survey
  global.json           pins .NET SDK 10.0.x
tools/bootstrap-fixtures.sh   pulls a curated set out of local Deadlock
tools/format-counts.csv       M0 survey output (regenerate with `just survey`)
Justfile                workspace task runner
```

## Public API (core)

```rust
use vpkmerge_core::{merge, MergeOptions, CollisionPolicy};

// Inspect a VPK's contents
vpkmerge_core::inspect("mod_dir.vpk")?;

// Preview path collisions without writing
vpkmerge_core::detect_conflicts(&["a_dir.vpk", "b_dir.vpk"])?;

// Merge. Default policy: LastWins.
merge(
    &["a_dir.vpk", "b_dir.vpk"],
    "combined_dir.vpk",
    &MergeOptions::default(),
)?;
```

`MergeOptions.collision_policy`:
- `LastWins` (default): later inputs override earlier ones at the path level
- `FirstWins`: first input wins
- `Error`: refuse to merge if any path appears in more than one input (CLI `--strict`)

Merge strategy: extract winners to a tempdir, then `valve_pak::from_directory` and `save`.

## CLI

```bash
cargo build --release -p vpkmerge-cli   # → target/release/vpkmerge

vpkmerge <output_dir.vpk> <in1_dir.vpk> <in2_dir.vpk> [more...] [flags]
```

| Flag | Meaning |
|---|---|
| `--strict` | Maps to `CollisionPolicy::Error`. Refuse to merge on any collision. |
| `-v`, `--verbose` | Print each path overridden by a later input. |
| `-h`, `--help` | Show usage. |
| `-V`, `--version` | Show version. |

Chunked inputs (`*_dir.vpk` + `*_000.vpk`, `*_001.vpk`, ...): pass only the `_dir.vpk`. Chunk files are read automatically when they sit alongside.

## GUI

Tauri v2 desktop app: drag-and-drop file input, visual conflict resolver, reorderable mod priority, custom title bar (`decorations: false`).

GUI default is **top of the list wins** (highest priority overrides), which maps to core `FirstWins` since the list is sent top-to-bottom. This intentionally differs from the core/CLI default (`LastWins`); the GUI exposes both as "Top wins" / "Bottom wins" plus "Refuse" (strict).

```bash
cd gui
pnpm install
pnpm tauri dev   # dev
pnpm tauri build # release bundle
```

Requires: Rust toolchain, Node 18+, pnpm, and the [Linux system deps Tauri lists for your distro](https://v2.tauri.app/start/prerequisites/#linux).

Frontend stack:
- Vue 3 + Vite 8
- Tailwind 4 via `@tailwindcss/vite`
- `@tauri-apps/api` for IPC, `@tauri-apps/plugin-dialog` for the native file picker

**Visual identity is ferry's paper/sepia palette**, not Grimoire's dark-orange. Tokens copied from `/home/esoc/ferry/app/main/src/vue_lib/`. Intentional aesthetic split.

## CI

GitHub Actions on push to `main` and PRs:

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings` (workspace `[lints]` sets clippy pedantic with a few allows: `missing_errors_doc`, `missing_panics_doc`, `module_name_repetitions`)
- `cargo test --workspace`
- Cross-OS sanity (Windows + macOS) for `core` + `cli`
- Vite frontend build for the GUI

## Conventions

- **Default collision policy is "last input wins".** Match the order from a mod manager's priority list.
- **Chunked VPKs are transparent.** Always pass the `_dir.vpk`; never enumerate chunks yourself.
- **No folder/glob input yet.** Positional args are individual file paths.
- **No em-dashes** in code, comments, commit messages, UI strings. Workspace-wide rule.
- **The C# CLI was retired** in favor of `vpkmerge-cli`. Don't reintroduce a .NET dependency to the shipped tool. The dev-time `tools/morphic-oracle/` C# project is exempt: it only generates committed golden artifacts that the pure-Rust tests then read. CI never runs `dotnet`.
- **morphic golden harness has three outcomes:** `pass`, `PENDING` (decoder not yet landed; visible but non-blocking), `FAIL` (regression in a working decoder; fails the build). Adding a new fixture for a not-yet-implemented format is safe; it appears as PENDING until that milestone lands.

## Known limitations

- `valve_pak::from_directory` walks the filesystem in OS-dependent order, so byte-exact output is not reproducible across runs. Same set of files, same content, different VPK hash. Fix needs an upstream patch.
- No streaming or progress callbacks yet; large merges block.

## morphic (Source 2 texture decoder)

Sibling crate, pure Rust. Targets the GUI's conflict-modal texture preview so two mods that touch the same `.vtex_c` can be compared visually without a .NET runtime. See [morphic/README.md](./morphic/README.md) for the API + milestone status. Names match VRF's `VTexFormat` (DXT1, ATI1N, BGRA8888, etc.).

Daily loop (requires [just](https://just.systems/) and .NET 10 SDK in `~/.dotnet/`):

```bash
just            # regenerate goldens via oracle, then run cargo test -p morphic
just survey     # resurvey Deadlock pak01; writes tools/format-counts.csv
just fixture materials/foo.vtex_c bc7   # add one fixture from local Deadlock
```

## Soundevents / KV3 (`.vsndevts_c`)

`morphic::kv3` is a generic binary KeyValues3 codec ported from ValveResourceFormat:
reads v1..=5 (incl. v5 two-buffer/LZ4), writes **v4 uncompressed** (no LZ4 *encoder*
needed; the engine reads either). `morphic::{decode_kv3_resource, encode_kv3_resource}`
wrap the resource envelope, preserving the format GUID and `RED2` on re-encode.

`vpkmerge-core::soundevents::SoundEvents` is the soundevents-aware layer (load from
file/VPK, JSON projection, `swap_vsnd`, `set_event_field`, re-encode). Exposed as
`vpkmerge soundevents <file> [--from-vpk <vpk>] [--swap-vsnd OLD=NEW] [--set EVENT/FIELD=N] [--encode OUT]`.

Built for a Grimoire per-ability sound picker (control `volume`/`pitch`/clip choice, not
just swap the audio). Full writeup + the pending in-game verification step:
[docs/spike-vsndevts-kv3.md](./docs/spike-vsndevts-kv3.md).

## Related

- `../grimoire/` is the mod manager that uses these VPKs. The user plans to eventually fold the GUI logic into the Grimoire desktop client; treat `gui/` as a prototype for that integration.
- `/home/esoc/ferry` is the source of the GUI's paper-themed tokens.
