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

## Sound swap (drop your own audio: `soundswap`)

`vpkmerge soundswap --from-vpk <pak> --clip <ENTRY.vsnd_c> --audio <FILE.mp3>
--encode-vpk <OUT_dir.vpk> [--loop auto|on|off] [--vpk-entry PATH]` replaces one clip's
audio with a user file. It reads the clip at `--clip` from the pak as its own **donor
template** (the same reuse-an-existing-asset model as `icon`), mints a new `.vsnd_c`
around the supplied MP3 via `morphic::encode_vsnd_c`, and packs it at the clip's entry
path so it overrides in place (the soundevent keeps pointing at the same path; the bytes
there are now yours). This is the Foundry sound-swap backbone: drop audio -> mint -> addon
VPK -> install as a managed local mod.

- **Loop is auto-detected, not guessed.** `--loop auto` (default) inherits the donor
  clip's own `m_vSound.m_nLoopStart` (`-1` = one-shot, `>= 0` = looping), so a `..._loop`
  / music clip stays looping and a VO line stays one-shot; `on`/`off` force it. The loop
  *points* don't survive a different-length substitution, so a swapped loop loops the
  whole new clip (handled by the minter). Reader: `morphic::sound::vsnd_looped`.
- **MP3 input, no ffmpeg.** v1 takes an MP3; the rate / channels / duration are parsed
  from the MP3 frame headers in pure Rust (`vpkmerge_core::parse_mp3_params`), so the tool
  stays a dependency-free standalone binary. Transcoding other formats to MP3 is a caller
  concern (a later enhancement can add it).
- **Trim + loudness gain, pure Rust, no decode.** `vpkmerge_core::{trim_mp3, apply_mp3_gain}`
  (`src/mp3.rs`) edit the raw MPEG frames: `trim_mp3(bytes, start_ms, end_ms)` byte-copies
  the frames in the window (frame-snapped, ~26 ms; kept audio bit-identical), and
  `apply_mp3_gain(bytes, gain_db)` shifts every Layer III granule's `global_gain` (the
  lossless mp3gain technique, ~1.505 dB/step, CRC recomputed) so a swap can match the
  volume of the sound it replaces with no transcode. Both feed the existing mint path.
  CLI: `soundswap --trim-start <MS> --trim-end <MS> --gain-db <DB>` (apply before mint, in
  both clip and event mode). The GUI's import editor (Foundry SwapPanel) authors these:
  a Web Audio waveform with draggable in/out handles, a play-selection preview, and a
  "match volume" normalizer that measures the import's vs. the donor clip's RMS in the
  renderer and passes the resulting `gainDb` through `foundry:swapSound`. A `--gain-db`-only
  CLI normalizer (auto-measuring loudness) is deferred since it would need an MP3 decoder
  in core; the GUI does the measurement. Verified: trim 200..700 ms -> 0.522 s frame-snap,
  gain +6/-6 dB byte-identical round-trip on a live LAME clip. In-game reconfirm pending.
- Core API: `vpkmerge_core::{mint_swapped_clip, parse_mp3_params, donor_is_looped}`
  (`src/soundswap.rs`). Verified on the live pak: loop auto-detect correct, the minted MP3
  round-trips byte-identical (re-extract via `catalog voiceclip`), substitution works.
  In-game rendering inherits the proven custom-`.vsnd_c` mint path; an independent in-game
  reconfirm of a `soundswap`-built addon is still pending.

**Event mode (swap by soundevent, not by clip).** The catalog surfaces *events*
(`catalog herosounds` / `catalog voiceline`), and most hero gameplay events are
**randomizer pools** (10-35 clips), so a single `--clip` swap only replaces 1 of N.
`vpkmerge soundswap --from-vpk <pak> --event <NAME> (--hero <CODE> | --soundevents
<ENTRY>) --audio <FILE.mp3> --encode-vpk <OUT_dir.vpk> [--pool all|collapse]
[--loop auto|on|off]` joins catalog -> swap: it resolves the event's clip pool from the
`.vsndevts_c` itself and handles the whole pool. `--hero <code>` resolves to
`soundevents/hero/<code>.vsndevts_c` (gameplay); pass `--soundevents <entry>` for VO or
any other tree.
- `--pool all` (default): mint your audio into **every** clip in the pool, each
  overridden in place. The event still randomizes but every option is your sound, so it
  always plays. The `.vsndevts_c` is untouched. Robust; pool clips that live in another
  pak are left unchanged and reported (`EventSwap.skipped`), never a hard failure.
- `--pool collapse`: mint **once** and rewrite the event's `vsnd_files` to that single
  clip (no randomization). Packs the edited `.vsndevts_c` + the one clip. Smaller. With
  `--vpk-entry <PATH>` it mints at a **new** clip path and repoints the event there
  instead of overriding the first pool clip in place. This is how you swap a sound whose
  clips are **shared** between heroes (e.g. `sounds/player/melee/shared/...` swing, used
  by all 39 heroes) for **one** hero only: the event (`Seven.Melee.Swing`) lives in that
  hero's `.vsndevts_c`, so repointing it to a fresh path leaves every other hero alone.
- Core API: `vpkmerge_core::swap_event_audio(vpk, sndevts_entry, event, mp3,
  looped_override, PoolPolicy, collapse_target)` -> `EventSwap`. Verified on the
  live pak (`tests/soundswap_event_live.rs`, gated on `DEADLOCK_PAK`): replace-all mints
  every present pool clip byte-identical to the input MP3, collapse rewrites the pool to
  one clip; both via `catalog voiceclip` re-extract. Single-`--clip` mode is unchanged.
  First real swap built + installed (Grimoire local mod "Seven Vine Boom Melee",
  `sound-swaps/seven-vineboom-melee/`): collapse + custom path puts a downloaded vine-boom
  MP3 on `Seven.Melee.Swing` only; in-game reconfirm pending. One event per invocation
  (multi-event-per-hero in one addon would need chained soundevents edits, a follow-up).

**Ability music map (which ult fits a music swap):** `examples/ult_sound_map.rs`
generates, from `scripts/heroes.vdata_c` (`ESlot_Signature_4` = ult) +
`scripts/abilities.vdata_c` (sound fields + `AbilityDuration`/`AbilityChannelTime`),
a per-hero catalog of each ultimate's swap-target **loop event** + intro/outro
bookends, tiered CHANNELED / SUSTAINED / TRANSIENT by music fit (40 ults: 21/9/10).
The loop event lives in the hero's `soundevents/hero/<code>.vsndevts_c`, so the
`SoundEvents` swap path above puts music straight onto an ult. Map + Grimoire
feature design: [docs/ability-music-mapping.md](./docs/ability-music-mapping.md)
(committed exports in `exports/ult-sound-map.{txt,json}`). `--json` for the machine
map. Extends to slots 1-3 via the other `ESlot_Signature_*` keys.

## Foundry catalog / voice-line index (`catalog`)

`vpkmerge_core::catalog` builds the Foundry sound-browse index from a Deadlock VPK:
`build_voiceline_index(vpk)` returns one `VoiceLine { event, hero, label, vsnd[],
duration, caption }` per VO sound event under `soundevents/vo/*.vsndevts_c` (against
live `citadel/pak01`: ~76K events / 56 speakers). `label` is the **searchable text**:
the event name spelled as prose (`bebop_ally_atlas_killed_in_lane_01_hero_3d` ->
`"ally atlas killed in lane"`); `event` is the verbatim swap target for the
soundevents layer; `vsnd` >1 entry == a randomizer pool. `hero` from each event's
`context_name`.

`CaptionDb` reads the compiled caption DB (`resource/localization/
citadel_generated_vo/citadel_generated_vo_<lang>.dat`, **VCCD v2**, keyed by
**CRC-32/ISO-HDLC** of the token via `caption_hash`; English+schinese in base
`citadel/pak01`, other langs in `citadel_<lang>/pak01`). **Caveat (scanned, corrected):**
Deadlock ships **no English subtitles for hero VO** (0/76K events resolve to caption
text); the ~5.7K non-empty captions are UI/store + authored NPC/story dialogue under a
separate token namespace, not joinable to swappable sound events. So `caption` is a
best-effort field, essentially always `None` for hero VO, and the descriptive name is
the search key. Exposed as `vpkmerge catalog voiceline --vpk <VPK> [--hero CODENAME]
[--search TEXT] [--limit N] [--json]` (table or machine array; logs a truncation note,
never a silent cap). Example: `examples/voiceline_index.rs`. Full design + findings:
[../grimoire/docs/foundry-tab-design.md](../grimoire/docs/foundry-tab-design.md).

## Foundry texture / icon index (`catalog texture`)

`vpkmerge_core::texture_catalog` is the visual counterpart to the voice-line
index: the browse backbone for the Texture and Item Foundry tabs.
`build_texture_index(vpk)` returns one `TextureEntry { path, category, hero,
label }` per `.vtex_c` (12,540 on live `citadel/pak01`), classified **purely from
the path** so the full index builds instantly with no byte reads.
`TextureCategory` (grounded in the live pak): `ability-icon`
(`panorama/images/hud/abilities/<hero?>/`, 232), `item-icon`
(`panorama/images/items|upgrades/mods_*|shop/`, 447), `hero-image`
(`panorama/images/heroes/`, 410), `hero-model`
(`models/heroes_staging|heroes_wip|heroes/<hero>/`, 2193 -- the skin/reskin/recolor
targets), `ability-vfx` (`materials/particle/abilities/<hero>/`, 456), and `other`
(8802). `hero` is the codename read from the path segment that encodes it; `label`
is the filename as prose (content hash + format token + hero prefix stripped:
`archer_bow_color_png_<hash>` -> `"bow color"`), the search key. `path` is the
verbatim icon-swap / recolor target.

Thumbnails decode on demand via morphic (the recolor-preview decode path):
`thumbnail_png(vtex_bytes, max_edge) -> Thumbnail { png, width, height,
source_*, format }` decodes the smallest mip whose longer edge is still >=
`max_edge` (the mip chain does the bulk of the downscale for free) then
box-filters to the exact target, so a 4K BC7 thumbnails in tens of ms.
`cache_texture_thumbnails(vpk, entries, out_dir, max_edge)` batches it to an
on-disk PNG set (filename = entry path with `/` -> `__`), returning a per-entry
`ThumbnailOutcome` (a texture that fails to decode is `Skipped`, never sinks the
batch). HDR (f16) sources are clamped to `[0,1]` as linear; the browse icons are
all LDR.

Exposed as `vpkmerge catalog texture --vpk <VPK> [--category <CAT>] [--hero
CODENAME] [--search TEXT] [--limit N] [--json] [--thumbs DIR [--thumb-size N]]`.
`--thumbs DIR` writes the PNG set + a `manifest.json` for every matching entry
(honors the filters, ignores `--limit`). Example: `examples/texture_index.rs`.
Full design: [../grimoire/docs/foundry-tab-design.md](../grimoire/docs/foundry-tab-design.md).

## Catalog on-disk cache (`catalog cache`)

`vpkmerge_core::catalog_cache` persists the built indexes so a UI does not rescan
on every launch (the voice-line scan alone touches ~76K events; cold build of both
indexes ~1.2s, warm load ~0.25s). `CatalogCache::new(dir)` then
`voicelines_cached(vpk)` / `textures_cached(vpk)` (-> `(items, was_hit)`) or the
flag-dropping `voicelines(vpk)` / `textures(vpk)` are load-or-build-and-store in one
call. Each index is one JSON file per kind (`voiceline.json`, `texture.json`),
written atomically (temp + rename) and wrapped in an envelope carrying the schema
version + the source build's [`BuildFingerprint`].

**Invalidation is by game build.** The fingerprint is the `_dir.vpk` file's byte
length + mtime: Steam rewrites that file whenever an update touches the pak (same
property the chunk-mtime update-diff tooling relies on), so the check is a single
`stat` (no VPK open) and never serves stale data after a real update. valve_pak does
not expose the V2 `tree_checksum`, so the file stat stands in; the only cost is a
needless rebuild if something bumps the mtime without changing bytes (a "verify game
files" pass). A corrupt / wrong-schema / stale-fingerprint cache is a **miss, not an
error** (the UI silently rebuilds). `CACHE_SCHEMA_VERSION` bump invalidates all
caches when the on-disk shape changes; `clear()` forces a rebuild.

Exposed as `vpkmerge catalog cache --vpk <VPK> [--dir DIR] [--clear] [--json]`:
warms both indexes, prints the fingerprint + per-index hit/miss. Example:
`examples/catalog_cache.rs`. Still open: a single combined catalog file (today it is
one-per-kind) and tying the texture-thumbnail dir to the same fingerprint.

## Hero display names (`catalog heroes`)

The catalog indexes key everything by engine **codename** (`hornet`, `vampirebat`);
a UI wants the **display name** (`Vindicta`, `Mina`). `vpkmerge_core::localization`
resolves them. The names are **not in pak01**: they live in loose Valve-KeyValues
`.txt` under `<game>/citadel/resource/localization/`. `build_hero_roster(vpk,
loc_dir, lang)` reads the codenames + availability flags from `scripts/heroes.vdata_c`
(inside the pak) and resolves each `hero_<codename>` token against
`citadel_gc_hero_names/citadel_gc_hero_names_<lang>.txt` (loose, located via
`localization_dir_for_pak`), returning `HeroInfo { codename, name, selectable,
in_development, disabled }`. On the live build that is 38 selectable / 59 total, with
the non-obvious mappings correct (`atlas`->Abrams, `forge`->McGinnis,
`familiar`->Rem, `orion`->Grey Talon, `synth`->Pocket, `frank`->Victor). A missing
localization tree degrades names to the codename (still returns the roster + flags).

`parse_kv_tokens(text)` is the generic reader (depth-2 quoted-string pairs, strips
the `:hint` type marker, handles BOM / `//` comments / `\"` escapes); `hero_name_tokens`
and `load_token_file` expose it. Exposed as `vpkmerge catalog heroes --vpk <VPK>
[--loc-dir DIR] [--lang L] [--all] [--json]` (selectable only by default; `--all`
includes in-dev / disabled). Example: `examples/hero_roster.rs`. This supersedes the
hardcoded 14-hero map in `examples/ult_sound_map.rs`. **Ability / item display names**
are the remaining piece: the strings are in the same localization tree but join to
their icons only through a fuzzy filename / vdata-node mapping (a later pass); the
catalog's filename-derived `label` already reads fine for those.

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

## Cubemap export (`.vtex_c` to `.hdr`)

`vpkmerge cubemap <file|entry> [--from-vpk <vpk>] --out-dir <DIR>` decodes a Source 2
cube texture at mip 0 and writes six Radiance `.hdr` faces (flat RGBE, no RLE) named
`px/nx/py/ny/pz/nz.hdr`, in morphic's cubemap storage order `[+X, -X, +Y, -Y, +Z, -Z]`,
which is also the order three.js `CubeTextureLoader` expects. Decode-only (nothing is
re-encoded or packed): built to ship real Deadlock IBL probes (the BC6H cube textures
under `materials/skybox/`, e.g. `sky_dl_dusk_ibl_exr_3dabb6cd.vtex_c`) to the grimoire
viewer's image-based lighting. f16 sources pass through as linear light; 8-bit sources
are treated as sRGB and linearized. Refuses non-cubemap textures (no `CUBE_TEXTURE`
flag). API: `vpkmerge_core::export_cubemap_hdr`, returning per-face mean luminance so
a caller can sanity-check orientation (`py` should be the brightest sky face).

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

## UV region masks (`model mask`)

The headless, Blender-free replacement for per-island skin masking. Blender's role
in per-part masking is purely mechanical (parse mesh -> select faces -> bake the
selection into a UV-space mask); `morphic::model::uvmask` does all three in pure
Rust off the already-decoded mesh. `segments(model, by, part)` partitions a model's
triangles into regions three ways: `island` (connected UV components, default),
`part` (one per mesh part), or `material`. `atlas_png` colors every region a
distinct hue (a picking atlas, the stand-in for Blender's viewport face-picker);
`mask_png` bakes selected region ids to a white-on-black PNG that the reskin
builders consume as a region selector in place of the AO-contrast heuristic.

UV islands are union-find over each vertex buffer's index graph: triangles sharing
a vertex index share that UV exactly, so a connected component is exactly an island
(a seam is where the exporter split a vertex). Region size is reported as true
**texel coverage** (rasterize, count unique texels), not summed UV area, because
hero UVs tile/mirror and inflate area well past 100%.

`vpkmerge model mask --vpk <VPK> [--base <VPK>] (--hero CODENAME | --entry PATH)
[--by island|part|material] [--part NAME] [--list] [--atlas PNG]
[--select ID... --mask PNG] [--resolution N]`. Core API:
`model_uv_segments` / `bake_uv_atlas` / `bake_uv_mask` (+ `hero_model_entry`).

**Caveat (Deadlock heroes):** `--by part` and `--by material` give clean masks;
island mode on hero **bodies** is streaky because the body mesh carries stretched
bridging UV triangles (the same overlapping/mirrored UVs that defeat Cycles
baking). Use `--part body` to drop the many small weapon islands, but expect
material/part masks to be the cleaner region selector on these specific assets.
Island mode is clean on models with proper UV charts.

**First reskin consumer:** `vpkmerge-core/examples/reskin_chrono_stained_glass.rs`
(Paradox "Stained Glass") drives its region selection off a baked **part** mask
instead of the AO heuristic. chrono's `body` and `headbase` parts share one
material/texture (`chrono_v2`), so the part mask is the *only* way to paint the
face differently from the torso in image space (leaded jewel glass on the body,
pale clear glass on the face); it also lead-blacks the dead texels so UV-edge mip
bleed reads as came. It bakes the masks in-process via the same
`morphic::model::{segments, mask_png}` the CLI uses (`--png` previews, second
positional arg bakes the addon VPK).

## Animation clip discovery (`model clips`)

Read-only companion to `model export`'s `--pose`/`--clip`: lists the animation
clips a model carries so a caller can pick a `CLIP[@FRAME]` instead of guessing.
Built for Grimoire's dev-only pose-authoring (hero-card snapshots): the static
pose baker already works, this is the missing discovery half.

`vpkmerge model clips --vpk <VPK> (--entry PATH | --hero CODENAME) [--base <VPK>]
[--json]`. Resolution mirrors `model export` exactly (same `--hero`
auto-discovery, same `--base` fallback), so the list is precisely what `export`
would pose/animate for the same selector. A clipless mesh skin falls back to the
base-pak donor's clips at the same entry; a model that embeds none and has no
clip donor returns an **empty list, exit 0** (not an error) - the same models
`--pose --require-pose` bails on. Default output is a table; `--json` emits an
array of `{name, frameCount, fps, durationSeconds, looping, default}`. `name` is
usable verbatim as `--pose <name>` / `--clip <name>`; `frameCount` bounds
`--pose name@N`; `default: true` flags the clip a bare `--pose` would bake (first
`DEFAULT_POSE_CLIPS` candidate the model carries). Core API:
`model_clips` / `hero_model_clips` (-> `ClipSummary`). Note: `--hero <codename>`
prefers the non-`heroes_wip` body, so WIP heroes still resolve to a clip-bearing
model; pass the explicit `heroes_wip` `--entry` to inspect the clipless rig.

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
[--edit-expr NAME=s/FIND/REPLACE/] [--targets all|body|weapons]
[--encode-vpk OUT_dir.vpk]`. `--list` surveys
shader/flags/texture channels **and decompiles every dynamic expression** (an
`expr NAME = SRC` line per `m_dynamicParams`/`m_dynamicTextureParams` entry).
Presets are modeled on shipped Valve materials
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
blobs (incl. negative literals: Valve emits `FLOAT; NEGATE`, never a folded
negative float, so neither does morphic). `morphic::vfx_expr::decompile` is the
in-Rust inverse (no .NET needed): it reconstructs source from the bytecode,
recovering attribute names from the material's `m_renderAttributesUsed`.
Round-trips on 1027/1044 shipped pak01 expressions (`compile(decompile(b))==b`);
the 17 hold-outs use local variables / multi-statement bytecode (opcodes
`0x08`/`0x09`, outside the grammar) and are reported as `<error>` rather than
guessed. Cross-checked against `tools/morphic-oracle dynexpr decompile` (VRF).

`--edit-expr 'NAME=s<D>FIND<D>REPLACE<D>'` edits an **existing** expression in
place: decompile -> literal substring substitution -> recompile (sed-style, any
delimiter `<D>`). This now works on blob-bearing shipped materials too (not just
adding expressions): an existing dynamic expression's bytecode IS a binary blob,
and `morphic::kv3::set_blob` -> `replace_blob_v5` swaps that blob byte-faithfully
while keeping the block compressed (the engine misreads a blobbed block flipped to
raw). The replace is content-keyed on the current bytecode, so it targets the right
blob even when the material carries several expressions (`countBlocks > 1`, e.g.
`ghost2_arm.vmat_c`'s two), and the new bytecode may differ in length (the whole
blob region is re-chunked into LZ4 frames). A length change moves `unc2`, so
`replace_blob_v5` also rewrites `sizeUncTotal@48` (= `unc1+unc2`) and
`sizeCompTotal@52`; a stale `@48` loads in morphic's lenient reader but crashes
Source 2 ("Bad KV3 data"), which is exactly what the first multi-blob test mod did
in-game (2026-06-16) before the fix. `verify_v5_size_invariants` now guards both
totals at pack time so a drift fails the edit instead of the game. `--set-expr`
overriding an existing param takes the same path. The multi-blob fix is guarded
offline by `verify_v5_size_invariants`; in-game rendering of the fixed path has
not been independently re-confirmed. Other
blobbed-`.vmat_c` edits that need the *re-encode* fallback (a tagless scalar with no
data lane) still fail loudly, since an uncompressed re-emit renders red wireframe.
480 shipped pak01 materials use these (430 on
`pbr.vfx`); engine-supplied entity attributes (`$ent_health`, `$ent_age`,
`$ent_origin`, ... + scene-side `$camera_origin`) enumerate from `strings` over
`game/citadel/bin/win64/client.dll`. `oracle dynexpr hash|brute` reverses
attribute tokens (murmur2, seed 0x31415926, lowercased, `$` included).

## NM animation pose codec (`.vnmclip_c`)

The newer Source 2 "NM" (motion-matching) clips store animation as a quantized
`m_compressedPoseData` blob. `morphic::model` decodes and **byte-faithfully
re-encodes** it, a port of VRF `ModelAnimation2/AnimationClip`:

- `decode_nm_clip(bytes) -> NmClip`: per-bone `NmTrack`s. Each track carries the
  static `TrackSettings` (per-channel `QuantRange` + the constant rotation) plus
  a per-frame `Vec` for every *animated* rotation/translation/scale channel
  (`None` when that channel is static, its constant living in the settings).
- `encode_compressed_pose(&NmClip) -> (data, offsets)`: the exact inverse.
- `decode_pose_stream(settings, data, offsets, frames)`: re-decode helper.

Layout: the stream is a flat little-endian `u16` array; `m_compressedPoseOffsets[f]`
is frame `f`'s starting word; within a frame each track emits, in
`m_trackCompressionSettings` order, 3 words for an animated rotation (the
"smallest three" packed quaternion), 3 for a translation, 1 for a scale.
Translation/scale dequantize as `start + (u16/65535)*length`. This is distinct
from the older `.vmdl_c`-embedded `ANIM`/`AGRP` clip decoder
(`morphic::model::animation`, the glb-export path); the static menu-pose reader
(`decode_nm_pose`/`bake_nm_pose`) is the constants-only subset.

Verified pak-wide (`tests/nm_clip_local.rs`, gated on `MORPHIC_MODEL_VPK`): all
9008 animated clips re-encode with translation/scale byte-exact and rotation
within 0.0012 rad; 90.7% are byte-for-byte identical. The rest differ only by the
smallest-three quaternion's inherent largest-component tie (an equivalent
encoding of the same rotation), not a codec error. CI round-trip on committed
`morphic/fixtures/nm/*.vnmclip_c` lives in `tests/nm_clip.rs`. Recon + format
writeup: [docs/handoff-nm-loose-clip-pose.md](docs/handoff-nm-loose-clip-pose.md).

**Editing an animated clip** writes the re-encoded stream back with
`morphic::patch_kv3_resource_blob` (-> `kv3::set_blob`) / `patch_kv3_resource_sole_blob`
(-> `kv3::set_sole_blob`): for a blobbed-LZ4 v5 block (`.vnmclip_c`) it recompresses
the pose blob into LZ4 frames, rewrites the per-frame size table in buf2's tail, and
keeps the block compressed (the engine misreads a blobbed block flipped to raw; same
constraint as the vmat recolor's `reassemble_blobbed_v5`). **Multi-frame blobs are
supported** (`replace_blob_v5` chunks the blob into 16 KB LZ4 frames and grows the
frame table + `unc2`/`sizeBlockCompressed`), so long clips whose pose stream exceeds
one frame -- `reload_idle` (61f), `sleep_idle` (121f), the stand idles -- are editable
in place, not just short single-frame ones. Single-blob only (every pose stream is one
blob). CI: `sole_blob_multi_frame_round_trips`. Examples: `yamato_custom_pose.rs` (static
pose edit via `m_constantRotation` patch) and `yamato_animated_taunt.rs` (animated
"bow" layered onto rotation tracks via the codec + blob splice). **In-game
confirmed (2026-06-13):** an edited animated pose stream loads and plays in the
live engine.

**Animated GLB preview:** `morphic::model::nm_clip_to_clip(clip, nm_skel,
model_skeleton, name)` converts a decoded `NmClip` into a glTF [`Clip`] mapped onto
the mesh skeleton by bone name (animated channels use their samples, static ones
hold their constant); attach it as the model's `animations` and `to_glb` emits a
playable animated GLB. `NmClip::duration` / `fps()` set the rate. This is the
grimoire animated-hero-preview primitive and the offline way to eyeball an edited
clip before installing. Example: `examples/nm_clip_preview_glb.rs`.

**Blender animation import:** `morphic::model::gltf_import` reads a
Blender-authored `.glb` animation back onto a slot's clip, the inverse of the GLB
preview. `read_glb_animation(glb, name)` groups the glTF animation's channels into
per-bone-name TRS keyframes (on the already-present `gltf` reader crate; bones map
by joint-node name, the contract the `.glb` writer upholds by naming each joint
after its bone). `apply_animation(clip, nm_skel, anim)` time-stretches those keys
onto the slot's fixed frame grid and maps them by name; `import_glb_onto_nm_clip`
is the end-to-end `bytes -> reencode_nm_clip` (v5 in-place). It honors the in-place
limits: rotations may be edited or **added** (a static bone becomes animated),
translation/scale edited only where the slot already animates them. CI:
`tests/gltf_anim_roundtrip.rs` (writer<->reader round-trip + map-and-edit against a
fixture). End-to-end pack: `examples/nm_clip_import_glb.rs` (read clip + skeleton
from a VPK, import the glb, pack an addon at the slot path(s), optional preview GLB).
This is the engine side of the SDK-free Blender authoring loop. **In-game confirmed
(2026-06-14):** a torso swivel hand-keyed on Yamato's rig in Blender (driven via
blender-mcp), exported to glTF, imported, and packed at the reload slot played in the
live engine. Blender preserves the per-bone coordinate frame our importer reads (a
null round-trip measured 0.51 deg mean rotation diff). Helper examples:
`gen_obvious_anim_glb.rs` (synthesize an exaggerated `.glb` with no Blender) and
`anim_glb_diff.rs` (the null-round-trip per-bone angle diff tool).

**End-state design** for hand-authoring animations in Blender (export rig to glTF,
key it, import + pack) and what's still missing (a v5 clip *encoder* that writes an
arbitrary-length pose blob in-engine, so translation/scale adds + frame-count
changes become engine-viable, vs. today's equal-length in-place patch):
[docs/anim-authoring-pipeline.md](docs/anim-authoring-pipeline.md).

## Related

- `../grimoire/` is the mod manager that uses these VPKs. The user plans to eventually fold the GUI logic into the Grimoire desktop client; treat `gui/` as a prototype for that integration.
- `/home/esoc/ferry` is the source of the GUI's paper-themed tokens.
