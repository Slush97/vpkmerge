# vpkmerge

Combine multiple Valve Pak (`.vpk`) files into one. Built for **Deadlock** modding: pre-merging several mods into one addon VPK consolidates them into a single load slot so a mod manager can resolve order and overrides up front.

Published at https://github.com/Slush97/vpkmerge (MIT).

## Layout

Rust Cargo workspace:

```
vpkmerge-core/        pure-Rust merge engine (lib, v0.6.0)
  src/lib.rs          public API: inspect / detect_conflicts / merge / split
vpkmerge-cli/         CLI binary `vpkmerge` on top of core (v0.5.0)
gui/
  src/                Vue 3 + Vite + Tailwind 4 frontend
  src-tauri/          Tauri v2 desktop app wrapping the same engine
morphic/              pure-Rust Source 2 decoder: .vtex_c + KV3 + .vmdl_c->.glb (lib, v0.2.0)
  src/                resource / kv3 / texture / model modules
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
`vpkmerge soundevents <file> [--from-vpk <vpk>] [--swap-vsnd OLD=NEW] [--set EVENT/FIELD=N] [--encode OUT] [--encode-vpk OUT_dir.vpk [--vpk-entry PATH]]`.

`--encode-vpk` re-encodes the edited file and packs it into a standalone addon VPK at its
entry path (defaults to INPUT under `--from-vpk`; `--vpk-entry` overrides, required for a
loose-file input). Built on `vpkmerge_core::pack`, which is the general primitive for
getting loose/generated files into a VPK so they can enter the merge pipeline.

Built for a Grimoire per-ability sound picker (control `volume`/`pitch`/clip choice, not
just swap the audio). Full writeup + the pending in-game verification step:
[docs/spike-vsndevts-kv3.md](./docs/spike-vsndevts-kv3.md).

## Texture recolor (`.vtex_c`)

`vpkmerge_core::recolor` hue-shifts a Source 2 texture in place: `morphic::decode` the top
mip, set every pixel's hue to a target (keeping each pixel's saturation and value), then
`morphic::replace_mip_chain` re-encodes the full mip chain in the texture's own format.
Packing the result at the source entry path overrides the base texture, no `.vmat_c` edit.
Hue is **set** (absolute), not rotated, so the same hue value matches the particle recolor;
neutral pixels (saturation 0) stay neutral. **LDR (8-bit) only** (HDR f16 is refused).

API: `recolor_texture_hue(bytes, hue) -> Vec<u8>` (full re-encode), `recolor_texture_image`
(fast, no re-encode, for a live UI preview), `recolor_texture_preview_png`, `inspect_texture`,
and `read_vpk_entry(vpk, entry)`.

Exposed as `vpkmerge texture <file|entry> [--from-vpk <vpk>] --hue <DEG> [--preview <PNG>]
[--encode OUT] [--encode-vpk OUT_dir.vpk [--vpk-entry PATH]]`. Built for the Deadlock
ability-VFX recolor (ult dragon, projectile self-illum), the texture half of the particle
recolor. Full writeup: [../grimoire/docs/ability-vfx-recolor.md](../grimoire/docs/ability-vfx-recolor.md).

## Model vertex-color recolor (`.vmdl_c`)

The **third** VFX recolor mechanism (after particle params + texture hue): some effects
bake their color into the mesh's per-vertex `COLOR` attribute (Paige's ult horse/knight),
which a material tint can only multiply, not replace. `vpkmerge_core::recolor_model_vertex_colors`
decodes each mesh vertex buffer, sets every `COLOR` vertex's hue to a target (keeping S+V,
the **same `set_hue` as the texture/particle recolor** so one hue lands all three), and writes
it back. Positions/normals/UVs/skin weights are byte-preserved. **In-game confirmed** (Paige
ult horse/knight read purple).

Two buffer encodings, both written **without re-encoding meshopt** (a re-encode is not
byte-compatible with the engine's meshopt decoder and renders garbled):
- **Uncompressed** buffer (Deadlock hero models): the `COLOR` bytes are patched in place in
  the file: output is byte-identical except the color lane (no container rebuild).
- **Meshopt** buffer (Deadlock `models/particle/*`): decoded, color-edited, then stored
  **uncompressed** with `m_bMeshoptCompressed` flipped to false in the `CTRL` registry
  (byte-faithfully, via `morphic::kv3::set_bools`). The engine reads uncompressed natively.

`morphic` primitives: `recolor_vertex_buffer`, `read_vertex_colors`,
`OnDiskBuffer::write_colors`, `VertexTarget::has_color`, `kv3::set_bools`.

Exposed as `vpkmerge model recolor [--list] --vpk <vpk> [--base <vpk>] --hue <DEG>
--encode-vpk <OUT_dir.vpk> <ENTRY>...` (multi-model -> one addon, mirrors `texture`).

**Finding the right model is the hard part:** an ult's rendered body is referenced by its
model particle (`.vpcf_c`), not named obviously. Paige's ult body is
`models/particle/bookworm_horse_knight.vmdl_c` (from `bookworm_ultimate_model.vpcf_c`), not
the `heroes_wip/bookworm/bookworm_horse*` models. Full writeup + workflow:
[docs/handoff-vertex-color-recolor.md](docs/handoff-vertex-color-recolor.md).

## Hero ability-VFX recolor (compose + prism)

`vpkmerge_core::hero_recolor` is the composition layer over the three mechanisms
above. A built-in per-hero **recipe** (`recipe_for`) pins which entries carry that
hero's ability color (particle prefixes + chromatic textures + tint materials +
vertex-color models), and one call recolors the whole set into a single addon VPK
that overrides the base in place. Pinned codenames (`pinned_hero_codenames`):
`bookworm` (Paige, full particles+textures+models), `necro` (Graves, +tint
materials), `yamato` (Yamato), `chrono` (Paradox), plus particle/texture/material
heroes `abrams`, `archer`, `digger`, `doorman`, `drifter`, `dynamo`, `familiar`,
`fencer` (Apollo), `frank`, `ghost` (Lady Geist), `haze`, `kelvin`, `nano`
(Calico), `lash`, `mcginnis` (McGinnis), `magician` (Sinclair), `pocket`,
`priest`, `tengu`, `unicorn` (Celeste), `viper`, `viscous`, `warden`, and
`werewolf`. Particle-only recipes are pinned for `astro` (Holliday), `bebop`,
`gigawatt` (Seven), `hornet`, `inferno` (Infernus), `mirage`, `punkgoat`, `shiv`,
`vampirebat` (Mina), and `wraith`. An unknown codename lists the pinned set.

Three CLI commands share these recipes:

- `vpkmerge recolor-hero --hero <CODENAME> --vpk <VPK> [--base <VPK>] --hue <DEG>
  [--saturation <SCALE>] [--brightness <SCALE>] (--encode-vpk <OUT_dir.vpk> |
  --preview-png <PNG>)`: recolor the whole VFX set to **one** absolute hue (the same
  `set_color` as the texture/model/particle recolors, so one value lands all three).
  `--preview-png` renders the recipe's representative texture as a fast swatch
  instead of baking.
- `vpkmerge prism --hero <CODENAME> --vpk <VPK> [--base <VPK>] --encode-vpk
  <OUT_dir.vpk> [--animated]`: instead of one hue, spread each effect's existing
  color/tint scalars across a **spectrum** (gradient stops become spectral ramps,
  themed by effect type). `--animated` adds a byte-faithful timing pass on
  high-visibility effects (glow/beam/trail/arc/slash): texture scroll repointed at
  particle age, scroll multiplier boosted, gradient stops retimed, so the spectrum
  sweeps over each particle's lifetime. Without it the prism is color-only (still
  reads as moving on heroes whose gradients already loop). `--animated` off is
  byte-identical to the static prism.
- `vpkmerge rainbow-scan --vpk <VPK> [--base <VPK>] [--hero <CODENAME>...]`: scan
  the pinned recipes (or the given heroes) and print a per-hero table classifying
  how well each suits rainbow treatment (`looped` > `animated` > `strong` >
  `gradient` > `static`). Run this first to pick the best prism candidate; Celeste
  (`unicorn`) is the richest.

Same engine as the GUI's Prism tab (`build_hero_prism_vpk`). In-game confirmed:
single-hue on Paige (purple); static + animated prism on Celeste and Paige. Source
of truth: [../grimoire/docs/ability-vfx-recolor.md](../grimoire/docs/ability-vfx-recolor.md)
and [docs/handoff-vertex-color-recolor.md](docs/handoff-vertex-color-recolor.md).

## Material shader-param styling (`.vmat_c`)

Deadlock heroes render through an NPR path built into `pbr.vfx`
(`F_USE_NPR_LIGHTING=1` on 502/605 hero materials), and the toon controls
(solid outlines, unlit) plus the specular side (sheen, glass) are all plain
per-material params. `vpkmerge_core::vmat_style` sets or inserts them
byte-faithfully (tagless 0/1 values fall back to a full re-encode on
non-blobbed materials) and packs an addon VPK.

`vpkmerge vmat --vpk <VPK> [--base <VPK>] (--hero CODENAME | --entry PATH...)
[--list] [--preset gem|glass|pbr|unlit|ink] [--tint COLOR] [--set-int NAME=V]
[--set-float NAME=V] [--set-vec NAME=X,Y,Z[,W]] [--set-expr NAME=EXPR]
[--targets all|body|weapons] [--encode-vpk OUT_dir.vpk]`. `--list` surveys
shader/flags/texture channels. Presets are modeled on shipped Valve materials
(gem sheen = `xmas_vindicta_dress`, glass = `viscous_body`); `pbr` turns NPR
lighting OFF for real reflections. Material paths use hero display names
(`vindicta`), not model codenames (`hornet`). Survey + probe plan:
[docs/spike-npr-toon-shading.md](docs/spike-npr-toon-shading.md); survey tool
`vpkmerge-core/examples/npr_vmat_survey.rs`.

`--set-expr` injects a **dynamic expression**: a per-frame snippet compiled by
`morphic::vfx_expr` to the engine's stack bytecode (`m_dynamicParams`), e.g.
`--set-expr 'g_vColorTint1=$ent_health<.4?float3(1,.1,.1):float3(1,1,1)'`.
Grammar: floats, `$attribute` reads (auto-registered in
`m_renderAttributesUsed`), the fixed builtin table (`sin`..`RemapValClamped`,
incl. `time()`, `lerp`, `float2/3/4`), arithmetic/comparisons, `?:`, `&&`/`||`,
swizzles, `exists()`. Verified byte-identical to Valve's compiler on shipped
blobs; round-trip gate = `tools/morphic-oracle dynexpr decompile` (VRF) must
print the source back. 480 shipped pak01 materials use these (430 on
`pbr.vfx`); engine-supplied entity attributes (`$ent_health`, `$ent_age`,
`$ent_origin`, ... + scene-side `$camera_origin`) enumerate from `strings` over
`game/citadel/bin/win64/client.dll`. `oracle dynexpr hash|brute` reverses
attribute tokens (murmur2, seed 0x31415926, lowercased, `$` included).

## Related

- `../grimoire/` is the mod manager that uses these VPKs. The user plans to eventually fold the GUI logic into the Grimoire desktop client; treat `gui/` as a prototype for that integration.
- `/home/esoc/ferry` is the source of the GUI's paper-themed tokens.
