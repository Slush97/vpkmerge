# Changelog

## v0.19.0

Caller-owned sidecar entries: embed and retire arbitrary files (identity records, manager metadata) in a VPK without the tool understanding them. Built to back Grimoire's self-identifying VPK imprinting. Contributed by @oldreceipt (#40), with a post-merge hardening pass (#41).

### CLI (`vpkmerge` 0.19)

- New `metadata` subcommand: patch identity into an already-built VPK. `--title`/`--author` (plus optional `--version`/`--description`/`--gamebanana-id`/`--source-url`/`--build-date`) generate a classic `addoninfo.txt`; repeatable `--extra-file ENTRY=PATH` embeds caller-owned files opaquely; repeatable `--drop-entry ENTRY` retires a superseded sidecar in the same repack. Whenever any typed field is given, `--title` and `--author` are required together, so a generated block can never carry empty identity fields over existing provenance.
- Bare merge gains the same repeatable `--extra-file ENTRY=PATH`. Extras behave as one final, highest-priority input: they win collisions, the override is counted and printed by `--verbose`, and `--strict` refuses it like any other conflict.
- The `metadata` report states what actually happened, never the request: entries actually dropped, drop requests that matched nothing, originals replaced, and an entry count taken from the VPK actually written.

### Library (`vpkmerge-core` 0.19)

- `embed_metadata(input, Option<&AddonMetadata>, extra_files, drop_entries, output) -> Result<EmbedReport>`: repack an already-built VPK with an optional typed `addoninfo.txt`, opaque extra files, and dropped entries. Drop matching happens on the same canonical path axis entries are written on (`./x`, `x`, and `.\x` name the same entry).
- `MergeOptions::extra_files`: opaque caller files embedded during a merge, riding the collision machinery as a synthetic highest-priority input. `MergeReport` gains `extra_replaced_paths`.
- One canonical entry-path normalization (`canonical_entry`) now backs traversal refusal, drop matching, collision detection, and disk writes, so those axes cannot drift apart.

## v0.18.0

Panorama (HUD/UI) workspace round-tripping, plus broader model decode coverage and a Victor VFX recolor fix. CLI-only release; all pre-existing commands are unchanged.

### CLI (`vpkmerge` 0.18)

- New `panorama` subcommand group for editing a HUD/UI mod's Panorama tree:
  - `panorama dump --vpk <VPK> --out-dir <DIR>` unpacks a VPK's Panorama resources into a readable, editable workspace (`.vxml` / `.vcss` / `.vjs` / `.vsvg` sources plus decoded soundevents). `--panorama-only` and repeatable `--prefix` narrow the dump, `--no-raw` skips the `_raw/` compiled copies, `--json` emits the full report.
  - `panorama build --workspace <DIR> --output <VPK>` repacks an edited workspace into an addon VPK, recompiling changed sources; `--allow-stale-raw` packs a changed decode-only sidecar's previous compiled bytes unchanged.

### Library (`vpkmerge-core` 0.18)

- New `panorama` module: dump a VPK's Panorama workspace and rebuild it into an addon VPK (`panorama::layout` owns the on-disk workspace layout).
- `soundevents`: extended to round-trip through the panorama dump/rebuild path.

### morphic (0.10.0)

- Decode the legacy VBIB/MBUF embedded-mesh layout, so older `.vmdl_c` meshes that use the pre-MBUF embedded vertex/index buffers now decode and export (improves `model export` and hero/prop preview coverage).
- `.vsnd_c` sound decode/encode helpers extended for the panorama soundevents path.

### Fixes

- `recolor-hero` / `prism`: drop a no-op material pin from Victor (`frank`). His Aura of Suffering is fully particle-driven (`g_vSelfIllumTint1 = $ILLUMCOLOR`, `g_vColorTint1 = $COLORTINT`), so the particle prefix pass already recolors the whole aura; `frank_painaura_sphere.vmat_c` carries no static tint constant, so the material patch was a no-op. Addons baked before the 2026-06-25 game update should be re-baked.

## v0.17.2

Drop your own audio onto any hero sound. The new `soundswap` command replaces a clip's (or a whole soundevent pool's) audio with a user-supplied MP3, packed into a standalone addon VPK that overrides in place.

Releases are now **CLI-only**: the Tauri desktop GUI is no longer built or attached to releases (it is a prototype, not the shipped tool). The `gui/` sources stay in the tree.

### CLI (`vpkmerge` 0.17)

- New `soundswap` subcommand:
  - Clip mode (`--clip ENTRY.vsnd_c`) mints a new `.vsnd_c` around the supplied `--audio FILE.mp3` using the clip itself as the donor template, packed at the clip's entry path.
  - Event mode (`--event NAME` with `--hero CODE` or `--soundevents ENTRY`) resolves the event's clip pool and handles all of it: `--pool all` (default) overrides every pool clip in place; `--pool collapse` mints once and rewrites the event to that single clip.
  - Loop is auto-detected from the donor clip (`--loop auto|on|off`), with pure-Rust MP3 trim/normalize: `--trim-start`/`--trim-end` (frame-snapped, bit-identical) and `--gain-db` (lossless mp3gain `global_gain` shift).

### Library (`vpkmerge-core` 0.17)

- New `soundswap` module: `mint_swapped_clip`, `swap_event_audio` (-> `EventSwap`, `PoolPolicy`), `parse_mp3_params`, `donor_is_looped`.
- New `mp3` module: `trim_mp3`, `apply_mp3_gain` (raw MPEG-frame edits, no decode).
- `morphic::sound::vsnd_looped` reads a clip's loop state for the auto loop default.

## v0.17.1

GLB export now keeps a model's additive glow/emissive overlays instead of dropping them, so a previewed material matches its in-game appearance (relevant to Grimoire's hero/prop preview). No CLI or library API changes; all commands are unchanged.

### morphic (0.9.1)

- `to_glb` preserves Source 2 additive glow/emissive overlay materials in the export instead of discarding them. Adds a gated real-data guard test (skipped in CI without `MORPHIC_MODEL_VPK`).

## v0.17.0

The **Foundry catalog** engine: build a searchable index of a Deadlock install's own assets, offline, from the user's `citadel/pak01`, plus the decode primitives a browse-and-audition UI needs. This is the backend for Grimoire's Foundry tab. All pre-existing commands are unchanged.

### CLI (`vpkmerge` 0.17)

- New `catalog` subcommand group (all support `--json`):
  - `catalog voiceline` lists VO sound events (one row per `soundevents/vo` event: event, hero, prose label, clip paths, duration), filterable by `--hero` / `--search`.
  - `catalog herosounds` lists the non-VO hero gameplay sounds (weapon / ability / movement / melee) from `soundevents/hero/<code>.vsndevts_c`, with a derived category, ability name, and slot. Filter by `--hero` / `--category` / `--search`.
  - `catalog voiceclip` slices the appended MP3 out of a `.vsnd_c` clip for audition (no decode), for both VO and gameplay clips.
  - `catalog texture` lists the texture/icon index (one row per `.vtex_c`, path-classified), with `--thumbs DIR` to decode a PNG set + manifest and `--path` for a single on-demand decode (the lightbox backbone).
  - `catalog cache` warms/refreshes both indexes on disk, keyed by the pak build fingerprint.
  - `catalog heroes` resolves codename -> in-game display name from `heroes.vdata_c` + the loose localization tree.

### Library (`vpkmerge-core` 0.17)

- New `catalog` module: `build_voiceline_index` / `build_hero_sound_index` (-> `VoiceLine` / `HeroSound` + `HeroSoundCategory`), `CaptionDb`, `caption_hash`.
- New `texture_catalog` (`build_texture_index`, `thumbnail_png`, `cache_texture_thumbnails`), `catalog_cache` (`CatalogCache`, build-fingerprint invalidation), and `localization` (`build_hero_roster`) modules.
- `extract_voiceclip_mp3` normalizes a `.vsnd` index path to the packed `.vsnd_c` and returns the clip's MP3 bytes.

### morphic (0.9.0)

- `extract_vsnd_mp3` slices the appended MP3 out of a `CVoiceContainerDefault` `.vsnd_c` (inverse of `encode_vsnd_c`).
- Texture decode fixes: correct non-power-of-two cropping (`crop_to_actual` + `TextureInfo.actual_width/height`) and YCoCg DXT5 decode (`TextureInfo.ycocg`), both verified pixel-perfect vs the VRF oracle.

## v0.16.0

Static posed bakes now anchor FeModel cloth bones to their true driver bones, so fabric (Pocket's scarf, coat hems, etc.) settles to its rest drape instead of detaching. Adds a read-only `model clips` discovery command and generalizes the soul-container clone path into a reusable GLB import primitive (urn swap, head-bone hat weld). Built for Grimoire's per-hero pose stills and custom-prop pipeline. All other commands are unchanged.

### CLI (`vpkmerge` 0.16)

- New `model clips` command: list the animation clips a model carries (name, frame count, fps, duration, looping, default) so a caller can pick a `CLIP[@FRAME]` for `--pose`/`--clip` instead of guessing. Resolution mirrors `model export` (`--hero` auto-discovery, `--base` fallback); a clipless mesh skin falls back to the base-pak donor's clips; a model with no clips and no donor returns an empty list at exit 0. `--json` emits an array; `--entry`/`--hero` select the model.
- `model export --pose`/`--clip` single-frame bakes now anchor FeModel cloth bones automatically (all four static-pose entry points route through the new anchoring); an export-time warning flags unresolved cloth/hair geometry. Animated exports keep their clip tracks unchanged.

### Library (`vpkmerge-core` 0.16)

- `model_clips` / `hero_model_clips` (-> `ClipSummary`) back the `model clips` command.
- `import_clone` over a `CloneTarget` generalizes the soul-container clone path: `soul_target` and `urn_target` presets plus a `NormalSynthesis` option retarget arbitrary model slots from a GLB. First new consumers: the Idol/urn objective swap (`examples/urn_import.rs`) and `hat_import`, which welds a custom GLB prop onto a Deadlock hero's head bone as an additive, rigid-skinned draw call with its own atlas material.
- `secondary_motion_pose_report` reports unresolved cloth/hair geometry after a pose bake.

### morphic (0.8.0)

- `model::femodel` decodes the FeModel `m_SkelParents` node tree from a model's PHYS block and walks each `$cloth_*` node to its terminal driver bone, exposing a `$cloth -> anchor` bone map (`ClothAnchors`). `decode()` populates `Model.cloth`; `finish_palette` rigidly carries each cloth root with its true anchor (falling back to the nearest non-secondary bone only when the model ships no FeModel). Reproduces the engine's settled rest drape for menu/idle snapshots; it does not run the solver, so live sway/collision under an arbitrary action pose is out of scope.
- New primitives `append_skinned_draw_call` and `replace_mesh_group_uncompressed` back the hat-weld path.

## v0.15.0

Two orientation knobs for `soul-container import` plus a mesh-group fix for pose export. Built for Grimoire's soul-container facing-yaw slider and cleaner per-hero pose stills. All other commands are unchanged.

### CLI (`vpkmerge` 0.15)

- `soul-container import` gains `--yaw <DEG>` and `--no-upright`. `--yaw` bakes a final-space (Source-Z) turn into the fitted geometry so the orb faces a chosen direction; it is intrinsic to the vertices and survives the particle orientation pass (unambiguous, unlike `--rotate`). Upright orientation is on by default (port of psyduck's recipe: clears `C_OP_PositionLock.m_bLockRot` and inserts an empty `C_OP_RemapTransformOrientationToYaw`, so the orb stands still and upright instead of tumbling with the control point); pass `--no-upright` to opt out. Byte-faithful KV3 v5 edits, idempotent. Both surface in the CLI JSON report.

### Library (`vpkmerge-core` 0.15)

- `soul_container` import exposes `yaw` and `orient_upright` options, reported in `SoulImportReport`.
- `model` export now honors the default mesh group. Source 2 models carry `m_meshGroups`, per-mesh `m_refMeshGroupMasks`, and `m_nDefaultMeshGroupMask`; the exporter now keeps a mesh only when `mask & defaultMeshGroupMask != 0` (mirroring the existing LOD0 filter). No-op for single-group heroes; the ~13 multi-bodygroup heroes now export the engine's default look instead of stacking every body-group variant (familiar 12->5, wraith 7->3, archer 9->6, ...).

## v0.14.0

A large modding-pipeline release: a Source 2 NM animation codec with a Blender-to-in-game authoring loop, cubemap-to-HDR export, GLB PBR-fidelity fixes, soul-container GLB import, a pure-Rust morphic authoring layer (texture/material/sound writers + UV region masks), and a music-pack pipeline. All existing commands (merge, recolor, prism, trippy, vmat, texture, model, soundevents, icon) are unchanged.

### CLI (`vpkmerge` 0.14)

- New `cubemap` subcommand: `vpkmerge cubemap <file|entry> [--from-vpk <VPK>] --out-dir <DIR>` decodes a Source 2 cube texture at mip 0 and writes six Radiance `.hdr` faces (`px/nx/py/ny/pz/nz`) in morphic's cubemap storage order (the order three.js `CubeTextureLoader` expects). Decode-only, built to ship real Deadlock IBL skybox probes to the grimoire viewer. f16 passes through as linear light, 8-bit is treated as sRGB; non-cubemap textures are refused.
- New `soul-container import <glb>` subcommand: build a soul-container override VPK from a user `.glb` (orient/glow options), a multi-material draw-call clone into the stock soul-container model with atlas albedo and fit/orient. Built for Grimoire drag-and-drop soul import.
- New `model mask` subcommand: segment a model's UV space (`--by island|part|material`), list regions, and bake a picking atlas or a white-on-black region-selector mask PNG, Blender-free. Texel-coverage metric (not summed UV area).

### Library (`vpkmerge-core` 0.14)

- New `soul_container` / `soul_import_clone` modules: clone a user `.glb` into the stock soul-container model (multi-material draw-call clone matching the shipped prop, atlas albedo, fit/orient, optional soul-glow recolor); one-call `import_soul_container_clone` bridge for Grimoire drag-and-drop import. Includes a draw-call index-offset fix and atlas-sampling fix.
- New UV-mask surface over morphic: `model_uv_segments` / `bake_uv_atlas` / `bake_uv_mask` (`SegmentBy` island/part/material), plus `export_model` and cubemap export `export_cubemap_hdr` (returns per-face mean luminance for orientation checks).
- Music-pack pipeline: `music-packs/<pack>` scaffolds (`source-tracks.json`, `download_songs.py`, manifest, README) plus generic manifest-driven builders (`build_music_pack` / `build_title_fight_pack`) and a `.vsnd_c` minting harness (donor `music_menu_lp.vsnd_c`).

### GLB export fidelity

- Roughness now reads from the blue channel of `g_tNormalRoughness` (alpha is a constant placeholder on Deadlock textures, so every textured material previously exported fully-rough/matte); normal-Z is reconstructed from X,Y, and constant metalness/roughness/color-tint recover from `TextureMetalness1` / `TextureRoughness1` / `g_vColorTint1`. Sheen reads `TextureSheenColor1 * tint` and binds `g_tSheen`; glass honors the authored `g_flIOR`. Verified across the hero roster (230+ normal-roughness textures).
- NPR shader-param extras (`F_USE_NPR_LIGHTING`, tint/rim masks) export as glTF extras for the grimoire viewer, plus post-serialize KHR material-extension injection (sheen).

### morphic (Source 2 codec)

- New pure-Rust authoring writers: `texture/vtex.rs` (encode RGBA8888 `.vtex_c` + from-PNG), `material/mod.rs` (compile/encode `pbr.vfx` `.vmat_c`), `sound.rs` (mint `.vsnd_c` from raw audio), `model/uvmask.rs` (UV segmentation + atlas/mask bake).
- New NM animation codec (`.vnmclip_c`): `decode_nm_clip` / `encode_compressed_pose` decode and byte-faithfully re-encode the quantized `m_compressedPoseData` pose stream (port of VRF `ModelAnimation2/AnimationClip`). Verified pak-wide: all 9008 animated Deadlock clips re-encode with translation/scale byte-exact and rotation within 0.0012 rad (90.7% byte-identical).
- Animated-clip editing: `patch_kv3_resource_blob` / `patch_kv3_resource_sole_blob` splice a re-encoded pose stream back into a blobbed-LZ4 v5 block in place (arbitrary length within one LZ4 frame), and `reencode_nm_clip` adds animated rotation channels (a static bone becomes animated). `nm_clip_to_clip` converts a decoded clip into a playable animated GLB. A restored end-of-block KV3 trailer and v4 binary-blob reading make re-encoded clips load in-engine. In-game confirmed: edited animated pose streams load and play live.
- `morphic` bumped 0.4.0 -> 0.7.0 across these.

### grimhud (new subsystem)

- `grimhud/`: a Grimoire-native Panorama HUD (`.vxml`/`.vjs`/`.vcss`) with a reposition catalog, enemy/ally HP color-warn, and an objective timer overlay, plus `tools/panorama-compiler/` (the proven Linux author -> resourcecompiler via Proton/CSDK -> VPK build path) and `tools/import-to-grimoire.mjs`.

## v0.13.0

Adds custom icon/hero-card import: build an addon that overrides Deadlock card art with a user PNG, no encoder that has to reproduce Valve's exact `.vtex_c` header. The new `icon` command reuses the base game's texture at each target path as a template: it reads that texture's format and dimensions, resizes the PNG to match, and splices it into the template's mip chain (the same in-place mechanism the recolor path uses), then packs every result at its entry path into one addon so it overrides in place. Built for Grimoire's Locker custom hero-card upload (one `--set` per card variant: minimap, small, card, card_critical, card_gloat, vertical). Existing commands are unchanged.

### CLI (`vpkmerge` 0.13)

- New `icon` subcommand: `vpkmerge icon --template-vpk <VPK> --set ENTRY=PNG [--set ENTRY=PNG ...] --encode-vpk OUT_dir.vpk`. Each `--set` reads the template `.vtex_c` at ENTRY from `--template-vpk` (e.g. the base `pak01_dir.vpk`), resizes the PNG (Lanczos3) to that texture's own dimensions, and packs the result back at ENTRY. HDR (`Bc6h`/`Rgba16161616F`) templates are rejected with a clear message; 8-bit card formats (`BGRA8888`, `RGBA8888`, embedded PNG, `BCn`) are supported and re-encoded in the template's own format.

### Library (`vpkmerge-core` 0.13)

- New `icon` module: `build_icon_from_template(template_vtex, png_bytes)` decodes + resizes a PNG to the template's dimensions and returns a new `.vtex_c` with the template's format/header/mip-count preserved (via `morphic::replace_mip_chain`); `png_to_rgba8_image(png_bytes, w, h)` exposes the decode+resize step. Promotes the `image` crate from a dev-dependency to a real dependency.

## v0.12.0

Adds material shader-param styling (`vmat`), exposes the trippy live preview to CLI callers, and grows the trippy style catalog from 8 to 14. The new `vmat` command edits `.vmat_c` shader params byte-faithfully and ships five presets (gem, glass, pbr, unlit, ink) modeled on shipped Valve materials, built on the finding that Deadlock heroes already render through an NPR toon path in `pbr.vfx` whose controls are plain per-material params. The v0.11.0 `trippy_preview_frames` pattern renderer (used by the GUI Locker tab) gains a sprite-sheet variant and a `trippy-preview` subcommand, so an external UI (Grimoire's Locker) can show an animated swatch of a trippy style without linking the library or touching a VPK. Six classic skin styles join the roster: camo, carbon, galaxy, halftone, lava, and vaporwave. Existing bake paths are unchanged: `trippy-skin`, `trippy-vfx`, `prism`, recolor, merge, and model outputs for the original 8 styles are byte-identical to v0.11.0.

### CLI (`vpkmerge` 0.12)

- New `vmat` subcommand: style hero materials by setting shader params directly. `vpkmerge vmat --vpk <VPK> [--base <VPK>] (--hero CODENAME | --entry PATH...) [--list] [--preset gem|glass|pbr|unlit|ink] [--tint COLOR] [--set-int NAME=V] [--set-float NAME=V] [--set-vec NAME=X,Y,Z[,W]] [--targets all|body|weapons] [--encode-vpk OUT_dir.vpk]`. `--list` surveys each material's shader, feature flags, and texture channels. Presets are modeled on shipped Valve materials (gem sheen on `xmas_vindicta_dress`, glass on `viscous_body`); `pbr` turns NPR lighting off for real reflections, `unlit`/`ink` lean into the toon path. Note material paths use hero display names (`vindicta`), not model codenames (`hornet`).
- New `trippy-preview` subcommand: render the procedural trippy pattern loop as one sprite-sheet PNG (frames left to right, width = frames x size). `--style`, `--phase`, `--scroll` (advances phase across the loop, mirroring the runtime UV-scroll speed), `--intensity` (pattern blend over the checkerboard base), `--frames` (1..=48, default 24), `--size` (16..=512 px, default 256), `--out <PNG>`. Pure pattern generation from the same function the skin/VFX bakes use; reads no VPK and runs in milliseconds.
- `trippy-skin`, `trippy-vfx`, and `trippy-preview` accept the six new styles everywhere `--style` is read.

### Library (`vpkmerge-core` 0.12)

- New `vmat_style` module: set-or-insert `.vmat_c` shader-param patching. Existing params are patched byte-faithfully in place; missing params are inserted into the KV3 arrays; tagless 0/1 int values fall back to a full re-encode on non-blobbed materials. Powers the `vmat` CLI command and packs results into an addon VPK.
- NPR shading survey: `docs/spike-npr-toon-shading.md` documents the `pbr.vfx` NPR vocabulary (`F_USE_NPR_LIGHTING` is on for 502 of 605 hero materials; outlines, unlit, sheen, and glass are all plain per-material params), with the survey tool at `examples/npr_vmat_survey.rs`.
- New `trippy_preview_sprite(style, phase, scroll, intensity, n_frames, size)`: the same frames as `trippy_preview_frames`, composed into a single sprite-sheet PNG. A 1-frame sprite is byte-identical to the first frame of `trippy_preview_frames`. The per-tile painter is shared between both paths, so the existing frames API is unchanged.
- Six new `TrippyStyle` variants, all seamless/tileable and animated by the same phase/scroll loop:
  - `camo` (aliases `camouflage`, `woodland`): quantized woodland blob camouflage with dark disruption patches and fabric speckle.
  - `carbon` (`carbonfiber`): plain-weave carbon fiber with cylindrical tow shading, fiber striations, and a sheen band that sweeps as it scrolls.
  - `galaxy` (`nebula`, `cosmos`, `space`): blue-violet nebula clouds with dust lanes and a twinkling star field (occasional warm stars).
  - `halftone` (`popart`, `comic`, `dots`): rotated Ben-Day dot grid over crawling pop-art color panels; pairs with the `vmat` `ink` preset for a full comic look.
  - `lava` (`magma`, `molten`): dark basalt plates with pulsing molten cracks (tileable Voronoi borders colored by the thermal ramp). On materials that expose the params, the skin bake also boosts `g_flSelfIllumScale1` and sets an orange Fresnel rim, the same byte-patch mechanism the holo style uses.
  - `vaporwave` (`synthwave`, `retrowave`, `outrun`): mirrored synthwave sunset with retro sun slats and a neon grid that rushes toward the horizon as it scrolls.
- Per-style VFX color-cycle gradients are pinned for the new styles (fire ramp for lava, woodland greens for camo, desaturated steel for carbon, blue-violet for galaxy, pop primaries for halftone, purple-pink-cyan for vaporwave), so `trippy-vfx` particles match the skin's theme.
- New internal tileable Voronoi field (nearest/second-nearest feature distance plus cell id), shared by the lava cracks and the galaxy star field.

### GUI (`vpkmerge` 0.12)

- GUI bundle version kept in lockstep at 0.12.0 (`tauri.conf.json`, `package.json`, `src-tauri/Cargo.toml`); no functional GUI changes.

## v0.11.0

Expands the ability-VFX recolor roster from 17 to 38 pinned heroes (Paradox / `chrono` pinned with a full particle-plus-texture recipe), adds two procedural VFX surfaces (`trippy-skin` / `trippy-vfx`), gives `prism` an `--animated` timing pass, extends `model edit` with draw-call/group geometry replacement, and hardens VPK extraction against path-traversal (Zip-Slip). All API changes are additive: existing recolor / prism / merge entry points and the GUI Prism tab are byte-compatible and unchanged, and default prism tuning is still byte-identical to prior releases.

### Security

- `vpkmerge-core` now rejects path-traversal in VPK entry extraction (Zip-Slip). Entry paths are stored verbatim in the archive, and `merge` / `pack` / `split` joined them with `tmp.join(entry)`, so a hand-crafted VPK carrying `..` segments or an absolute path could plant a file outside the temp dir. A new internal `safe_join` refuses absolute paths, `..` segments, and drive/volume components (both separators), then joins only normal segments. It is applied at all three extraction sites (and transitively the soundevents / texture / model `--encode-vpk` flows). Verified against 130,606 real entry paths (full base-game pak01 plus real mod VPKs): 0 rejected, valid entries resolve byte-for-byte as before, only tampered archives now error.

### CLI (`vpkmerge` 0.11)

- `recolor-hero`, `prism`, and `rainbow-scan` gain 21 newly pinned heroes, bringing the pinned roster to 38: `chrono` (Paradox), `archer`, `bebop`, `digger`, `doorman`, `drifter`, `dynamo`, `familiar`, `frank`, `haze`, `hornet` (Vindicta), `kelvin`, `mirage`, `priest`, `punkgoat`, `shiv`, `tengu`, `viper`, `viscous`, `warden`, and `werewolf`. Paradox carries a full recipe (ability particle prefixes plus her time-stop bubble FX textures); the others extend particle / recolor coverage. The `recolor-hero` help and not-found error still list the pinned set dynamically.
- New `trippy-skin` subcommand: paint a hero skin with a procedural pattern plus runtime VMAT UV-scroll. `--style {confetti,liquid,moire,kaleido,holo,glitch,thermal,gradient}` (default `confetti`), with `--intensity` (texture blend strength), `--scroll` (UV-scroll speed scale), `--phase` (pattern/hue offset), and `--targets {all,body,weapons}`. Reads from `--vpk` (with `--base` fallback) and packs the result into a single `--encode-vpk` addon.
- New `trippy-vfx` subcommand: paint and animate a hero's ability/weapon VFX with the same procedural themes over the pinned recipes. Adds `--animation-style {off,sweep,loop,cycle}` (default `cycle`) and `--animation-intensity` on top of the `trippy-skin` style/phase flags, and `--targets {all,abilities,weapons}`. `sweep` retimes safe texture-scroll/gradient fields, `loop` also loops color gradients, `cycle` also inserts runtime color-cycle operators where safe.
- `prism` gains `--animated`: a byte-faithful timing pass on high-visibility effects (glow/beam/trail/arc/slash) that repoints texture scroll at particle age, boosts the scroll, and retimes gradient stops so the spectrum sweeps over each particle's lifetime. `--animation-intensity` and `--animation-style {sweep,loop,cycle}` tune it. Without `--animated` the prism is color-only and byte-identical to prior releases.
- `model edit` gains draw-call/group geometry editing: `--list-drawcalls` (`--json` for machine-readable) enumerates renderable draw calls; `--group <NAME>` / `--material <SUBSTRING>` select a semantic group (gun, hair, dress, body, hands, legs); `--export-group-glb` exports a selected group as one isolated `.glb`; `--replace-group <GROUP>` and `--replace-part <MESH> --from-glb <FILE>` (Tier 1d) replace a part's geometry wholesale with a new mesh of any vertex/index count; `--remove-material <MATERIAL>` drops a part's draw calls so it stops rendering.

### Library (`vpkmerge-core` 0.11)

- New `recipe_for` arms and `pinned_hero_codenames` entries for the 21 heroes above (notably `chrono`, Paradox, with chromatic textures). `PrismGradient`, `PrismTuning`, and all existing recolor / prism entry points are unchanged.
- The trippy module is promoted to public API: `trippy_skin_to_addon`, `trippy_ability_vfx_to_addon`, `trippy_preview_frames`, and `TrippyStyle`.
- Animated-prism timing is exposed: `animate_particle_timing_bytes`, `PrismAnimationStyle`, `ParticleTimingAnimationStats`, and `insert_color_cycle_operator_with_tuning`. The static prism path is unchanged.
- Model part/group editing: `inspect_model_parts`, `export_model_group_glb`, `replace_model_group`, and the part-inspection types (`ModelPartInspection` / `ModelPartSelector`, `DrawCallSkinInfo`, `ResolvedResource` / `TextureParam`, `SuggestedPartGroup`, `ModelDrawCallInspection`). Built on new `morphic` primitives `replace_mesh_group`, `read_edited_primitives`, `EditedPrimitive`, `PrimitiveSelection`, and `Material::uses_vertex_color`.

### GUI (`vpkmerge` 0.11)

- New hero-locker / trippy preview tab (`LockerTab.vue`): browse pinned heroes and preview their trippy VFX from the desktop app.
- The new heroes are wired into the GUI's hero-label map so they show proper names in the Recolor / Prism / Locker tabs. GUI bundle version bumped to 0.11.0 across `tauri.conf.json`, `package.json`, and `src-tauri/Cargo.toml`.

## v0.10.0

Expands the ability-VFX recolor roster and adds custom color ramps to the `prism` rainbow. Nine more heroes are pinned with recolor recipes, and `prism` can now spread each effect across a named preset or a hand-written gradient instead of the full spectrum. This release also re-aligns the GUI bundle version, which had lagged since v0.6.0.

### CLI (`vpkmerge` 0.10)

- `recolor-hero`, `prism`, and `rainbow-scan` gain nine new pinned heroes: `abrams` (Abrams), `astro` (Holliday), `fencer` (Apollo), `ghost` (Lady Geist), `nano` (Calico), `lash` (Lash), `mcginnis` (McGinnis), `magician` (Sinclair), and `pocket` (Pocket). `astro` is particle-only; the other eight pair the standard particle roots with the hero-specific chromatic textures isolated by the texture audit (shared masks, normals, AO, and cross-hero defaults are deliberately excluded). The `recolor-hero` help and the not-found error now list the pinned set dynamically instead of a hard-coded subset.
- `prism` gains `--gradient <SPEC>`: spread each effect across a custom color ramp instead of the full rainbow. `SPEC` is either a built-in preset (`fire`, `ice`, `toxic`, `sunset`, `ocean`, `neon`, `gold`, `void`) or a stop list `pos:hue[:sat],...` (position 0..1, hue in degrees, optional saturation 0..1). `--hue-offset` / `--saturation` / `--brightness` still apply on top of the sampled gradient.

### Library (`vpkmerge-core` 0.10)

- New public API for custom prism ramps: `PrismGradient` (with `from_spec` / `preset` / `from_stops`), `GradientStop`, `MAX_GRADIENT_STOPS`, and `PRISM_PRESET_NAMES`. `PrismTuning` gains a `gradient: Option<PrismGradient>` field; `None` reproduces the canonical rainbow byte for byte, so existing callers (e.g. the GUI Prism tab) are unchanged. Hue interpolates along the shortest arc so a ramp crossing the 360/0 boundary travels the short way.
- Nine new `recipe_for` arms / `pinned_hero_codenames` entries (see above). The recolor entry points (`recolor_hero_to_addon`, `recolor_hero_preview_png`) build their "no recipe for codename" error from `pinned_hero_codenames()` rather than a stale literal.

### GUI (`vpkmerge` 0.10)

- The GUI bundle version (`tauri.conf.json`, `package.json`, `src-tauri/Cargo.toml`) is bumped to 0.10.0. These had lagged at 0.6.0 since v0.6.0, so the deb / AppImage / dmg / msi bundles were mislabeled; they now read the real version. The new heroes are wired into the GUI's hero-label map.

## v0.9.0

Adds spectrum tuning to the `prism` rainbow recolor. The canonical rainbow can now be rotated to a different start hue and scaled in saturation and brightness, so a UI can offer "rotate / desaturate the rainbow" without losing the per-effect spread. Defaults reproduce the v0.8 prism byte for byte. Also wires Yamato into the prism path's chromatic-texture set.

### CLI (`vpkmerge` 0.9)

- `prism` gains `--hue-offset <DEG>` (rotate the whole spectrum's start hue; the per-effect spread is unchanged, just shifted), `--saturation <SCALE>`, and `--brightness <SCALE>` (scale the spectrum, e.g. a pastel rainbow). All three default to the canonical rainbow, so omitting them reproduces the previous output.

### Library (`vpkmerge-core` 0.9)

- New `PrismTuning { hue_offset, saturation, brightness }` and `prism_recolor_hero_to_addon_tuned`, threading the tuning through every spectrum site (particle gradient stops + color fields, textures, materials, models, and the Yamato shadow-band texture). The existing `prism_recolor_hero_to_addon` delegates with `PrismTuning::default()`, so callers that do not expose the knobs (e.g. the GUI Prism tab) are unchanged; default tuning is byte-identical to v0.8.

## v0.8.0

Lands the Deadlock ability-VFX recolor toolkit and the first part-level model edits, plus a large texture-encode speedup. A VFX effect carries its color on up to three axes (particle params, textures, and baked mesh vertex colors); this release can retint all three to one target color, exposes a per-hero "recolor recipe" that drives them together, and adds saturation and brightness control on top of hue. On the geometry side, `model edit` graduates from whole-model reshape to removing and replacing individual mesh parts, and `model export --pose` now poses the WIP heroes whose menu pose ships as a loose motion-matching clip. The recolor and part-edit paths are confirmed in-game.

### CLI (`vpkmerge` 0.8)

- New `texture` subcommand. Recolors a Source 2 `.vtex_c` by setting every pixel's hue (optionally scaling saturation / brightness) to a target, re-encodes the full mip chain in the texture's own format, and packs the result at its entry path so it overrides the base texture with no `.vmat_c` edit. `vpkmerge texture <file|entry> [--from-vpk <vpk>] --hue <DEG> [--saturation <S>] [--brightness <V>] [--preview <PNG>] [--encode OUT] [--encode-vpk OUT_dir.vpk [--vpk-entry PATH]]`. LDR (8-bit) textures only.
- New `recolor-hero` subcommand. Applies a hero's whole recolor recipe (its particle prefix plus any color-bearing textures and vertex-color models) to one target color in a single addon VPK. `--preview-png` renders a fast PNG swatch of the recipe's representative texture (~170ms, no bake) for a live UI preview. Ships the first recipe, Celeste (`unicorn`), a particle-only hero: 228 `.vpcf_c` under `particles/abilities/unicorn/`, no color textures or baked vertex colors.
- New `model recolor` subcommand. Recolors a model's per-vertex `COLOR` attribute (the color some effects bake into the mesh, e.g. Paige's ult horse/knight, that a material tint cannot reach), writing the result without re-encoding meshopt. `vpkmerge model recolor [--list] --vpk <vpk> [--base <vpk>] --hue <DEG> [--saturation <S>] [--brightness <V>] --encode-vpk <OUT_dir.vpk> <ENTRY>...` (multi-model to one addon, mirrors `texture`). The same hue target lands all three recolor axes.
- `model edit` gains part-level edits beyond the v0.6 reshape:
  - `--remove-material <MATERIAL>` removes a mesh part by material (`--list-drawcalls` enumerates them first). Done by zeroing the matching draw calls' index count in a byte-faithful KV3 re-wrap, so the engine loads the edited model with the part gone and no ERROR substitution. Confirmed in-game.
  - `--replace-part <MESH> --from-glb <FILE>` replaces an existing mesh part's geometry in place with a new mesh of any vertex/index count, via block swaps and in-place KV3 scalar edits (no container rebuild).
  - `--export-glb <FILE>` / `--from-glb <FILE>` round-trip a model's mesh through a glb for Blender reshaping: export the editable mesh, move its vertices in Blender (topology preserved), and re-import to write a reshaped addon VPK. `--glb-mesh <NAME>` selects the mesh.
- `model export --pose` now poses WIP heroes (Apollo, Billy, Celeste, Mina, Paige, Rem) whose menu pose ships as a loose `.vnmclip_c` (the newer NM motion-matching format) rather than an embedded clip, so `--require-pose` succeeds for them instead of dropping to a 2D portrait. Also drops Viscous's hidden alt-form Goo Ball mesh from a static posed export (it stayed full-size and swallowed the body).
- `--saturation` / `--brightness` (default 1.0) added to `texture`, `recolor-hero`, and `model recolor`, so a recolor can reach pastels and stop washing out pale source art.

### Library (`vpkmerge-core` 0.8)

- New `recolor` module: `recolor_texture_hue` / `recolor_texture_image` / `recolor_texture_preview_png` (texture), `recolor_model_vertex_colors` (per-vertex COLOR), `inspect_texture`, and the shared `Recolor { hue, saturation, value }` target plus `read_vpk_entry`. One `set_color` drives the texture, particle, and vertex-color paths so a single target lands all three.
- New `hero_recolor` module: `recolor_hero_to_addon` composes a hero's particle + texture + model recolor into one addon VPK from a `HeroRecolorRecipe` (`recipe_for`), `recolor_particle_bytes` retints `.vpcf_c` color params via the byte-faithful KV3 scalar patch, and `recolor_hero_preview_png` renders the live-UI swatch. Reports via `HeroRecolorReport`; `ModelRecolorStats` on the model path.
- New geometry edits on the `model edit` core: `remove_model_material` / `model_draw_call_targets` (draw-call removal), `replace_model_part` (replace a part with a new mesh), plus the glb reshape round-trip. The lossy value-tree KV3 writer is NOT engine-valid for model blocks (it drops value flags and flattens typed-array tags, so the engine substitutes its ERROR model), so every model edit re-wraps the block uncompressed and patches in place.
- WIP-hero posing reads the loose NM clip as a third pose source (after embedded clips and the base-pak donor): `bake_loose_nm_pose` resolves `<dir>/clips/<cand>.vnmclip_c`, reads the referenced `.vnmskel_c`, and bakes the single static card-pose frame onto the model skeleton by bone name.

### morphic (0.4)

- BCn block encoding now runs in parallel across cores. BCn 4x4 blocks carry no cross-block state, so each encode splits into block-row strips compressed with rayon; output is byte-identical to the single-call encoder. The dominant cost in an ability-VFX recolor (Paige's 9 BC7 maps, one 4096x4096) drops from ~27s to ~3.6s on 16 cores (7.4x). Applied to the RGBA8-surface encoders (BC1/BC3/BC7).
- New byte-faithful KV3 editing primitives for model and particle edits. `patch_kv3_resource_scalars(file_bytes, edits)` patches integer scalar fields in a resource's KV3 `DATA` block, located by path (`&[(Vec<kv3::Seg>, i64)]`). Unlike `encode_kv3_resource` (which rebuilds `DATA` from a value tree and so downgrades v5 to v4 and drops value flags + typed array tags, fatal for particles/models), it `rewrap_uncompressed`s the block (preserving v5 framing, flags, and typed tags), applies `kv3::set_scalars`, and rebuilds the container; the lossy re-encode path renders the engine's red error particle. Lower level: `kv3::set_scalars` / `kv3::set_bools` (in-place scalar/bool set), `kv3::neutralize_draw_calls`, and `kv3::rewrap_uncompressed`.
- New vertex-color primitives behind the model recolor: `recolor_vertex_buffer`, `read_vertex_colors`, `OnDiskBuffer::write_colors`, `VertexTarget::has_color`. Both buffer encodings are written without re-encoding meshopt (a re-encode renders garbled in the engine): the uncompressed buffer patches the COLOR bytes in place; the meshopt buffer is decoded, color-edited, and stored uncompressed with `m_bMeshoptCompressed` flipped to false (byte-faithfully via `kv3::set_bools`).
- New Tier 1 model-editing pipeline: a meshopt index encoder (the inverse of the decoder), new-mesh assembly from a glb (`mesh::assemble_to_layout`, skin-weight encode), bone-palette remapping (`skeleton::{invert_remap, localize_joints}`), and `topology::replace_mesh_part`, which splices an assembled mesh into the container via block swaps.
- New NM (motion-matching) format support: `model::nm` (`decode_nm_skeleton` / `decode_nm_pose` / `bake_nm_pose`) reads the static single-frame card pose from a loose `.vnmskel_c` + `.vnmclip_c`, and `pose::bake_pose_named` maps it onto the model skeleton by bone name.
- The GLB writer drops Viscous's hidden-by-default alt-form Goo Ball mesh parts (matched on part name and the `viscous_ball` material token) alongside the existing NPR effect shells, so they stop swallowing the body in a static posed export.

## v0.7.0

Hardens the posed-export path that Grimoire's Locker hero previews depend on. Hero body-model discovery is now deterministic, `--pose` can refuse to emit an unposed model, and Deadlock's comic-outline shells no longer leak into the GLB as a white halo.

### CLI (`vpkmerge` 0.7)

- New `--require-pose` flag on `model export`. With `--pose`, errors out instead of baking a static bind/T-pose when the model carries no menu/idle pose clip (WIP heroes ship the rig but no baked clips). Lets a caller fall back to a 2D portrait rather than show an unposed hero.
- `--hero` body-model discovery is now deterministic: it picks the highest `_vN` model directory consistently instead of relying on VPK enumeration order, so the same hero resolves to the same body model every run.

### Library (`vpkmerge-core` 0.7)

- Pose selection can now signal "no real pose available" so callers can require one (see `--require-pose`); a clipless model no longer silently degrades to the bind pose when a pose was demanded.

### morphic (0.3)

- Drop the non-renderable comic-outline (`*jitter*`) effect shells during GLB export alongside the existing `*_outline` / `*_glow` shells, so they stop collapsing into an opaque white halo in static previews.

## v0.6.0

Adds a static *posed* model export for still previews (a hero card), and stops Deadlock's non-renderable effect shells from leaking into the GLB. `vpkmerge model export --pose` bakes a single animation frame into the mesh and drops the skeleton, skin, and clips, producing a plain posed mesh the size of a static prop. Built for Grimoire's Locker hero previews, where a lightweight still beats shipping a multi-megabyte animated rig.

Also lands the first geometry-editing path: `vpkmerge model edit` reshapes a model's existing vertices (scale and/or translate, topology preserved), re-encodes the meshopt vertex buffers, and packs the result into an addon VPK. Confirmed in-game with a scaled-up hornet body model.

### CLI (`vpkmerge` 0.6)

- New `--pose [CLIP[@FRAME]]` flag on `model export`. Bakes one frame into the vertices and writes a static `.glb` with no skeleton, skin, or animation. Omit the value to try the default menu/roster poses (`ui_hero_pose`, `hero_pose`, `hero_roster_pose`, `hero_roster_ready`, `ui_hero_select`) then generic idles (`idle_loadout`, `primary_stand_idle`) in order, since menu-pose naming differs across heroes; pass a clip name and optional `@frame` to choose explicitly. Mutually exclusive with `--clip` / `--no-anim`.
- New `model edit` subcommand. Reshapes a model's existing geometry by vertex displacement: uniform `--scale` about each part's centroid plus an optional `--translate x,y,z`, with topology preserved. Writes the edited `.vmdl_c` into a standalone addon VPK via `--encode-vpk` (entry path defaults to `--entry`; `--vpk-entry` overrides). `--list` enumerates the editable vertex buffers (mesh part, block index, vertex count) and exits without editing. Reads the mesh from `--vpk` (or `--base` when the skin is texture-only).

### Library (`vpkmerge-core` 0.7)

- `AnimOptions` gains an optional `pose: Option<PoseSelection>`; when set, the export bakes a single posed frame and emits a static mesh, overriding clip / no-anim selection. `PoseSelection { clips, frame }` and `DEFAULT_POSE_CLIPS` are exported; an empty clip list uses the defaults.
- New `edit_model_geometry` plus `GeometryEdit { scale, translate }`, `GeometryEditReport`, and `model_vertex_targets`: decode a `.vmdl_c` from a VPK (or base pak), apply a vertex-displacement edit, re-pack the edited model into an addon VPK, and report the edited parts/buffers/vertices. `model_vertex_targets` lists the editable buffers without editing.
- **Skin mods are posed from the base game's clips.** Skin VPKs ship the mesh + rig but no animation clips, so when the exported model carries none of the requested clips, the same entry is read from the base pak (`--base`) and its clip is mapped onto the skin by bone name (`morphic::model::bake_pose_from`). Same hero, same rig, so no cross-hero retargeting. A hero with no clips anywhere (e.g. an unfinished model) falls back to the bind pose.

### morphic (0.3)

- New `bake_pose(model, clips, frame)`: folds one animation frame into the mesh by linear-blend skinning (each hero's own clip on its own skeleton, so no retargeting), returning a static `Model` with no skeleton, skin weights, or clips. Vertex buffers with no joints pass through unchanged, so a prop posed by its own bone (a held weapon) follows the pose while truly static decor stays put.
- New `bake_pose_from(model, donor, clips, frame)`: bakes `model`'s mesh using a donor model's clips, mapped onto `model`'s skeleton by bone name. For posing a clipless skin with its base-game hero's clip.
- The GLB writer now drops Deadlock's additive glow-effect shells (mesh part `ghost_glow`, `*_glow` materials) alongside the existing inverted-hull `*_outline` shells. As plain glTF geometry both collapse to an opaque "white halo" over the model; their in-game NPR shaders are a renderer-side concern. `*_noglow` materials are kept.
- The vertex decoder now handles 8-influence skinning, unblocking the current Dynamo and Apollo (`fencer`) body models, which previously failed with `unexpected BLENDWEIGHT format`. Their meshes pack up to 8 bones per vertex (an 8-wide `BLENDINDICES` paired with an `R16G16B16A16_UNORM` weight stream of 8 `u8`s). Since the glTF pipeline is fixed at 4 influences, a vertex carrying more keeps its 4 highest-weight bones with the weights renormalized to sum 1; 4-influence meshes are unchanged.
- Texcoord decode now accepts the 1-component `R32_FLOAT` format (V zero-filled), which unblocks the `prof_dynamo` staging model that paired it with the 8-influence skinning above.
- New meshopt vertex encoder (codec v1), the inverse of the existing decoder, plus a `model::edit` module (`vertex_targets`, `read_vertex_positions`, `replace_vertex_positions`). Together they round-trip an edited vertex buffer back into the Source 2 `VBIB` block, preserving topology and every non-position attribute. This is the primitive behind core's `edit_model_geometry`.

## v0.5.0

Adds a Source 2 model exporter. `vpkmerge model export` turns a Deadlock hero `.vmdl_c` (from the base pak or a skin VPK) into a textured, skinned, animated `.glb`, decoded entirely in pure Rust (a faithful port of ValveResourceFormat, no .NET or C runtime). The exported model carries the hero's own animation clips on its own skeleton, so a viewer can play its idle without any cross-hero retargeting. Built for Grimoire's hero/skin preview.

### CLI (`vpkmerge` 0.5)

- New `model` subcommand. `model <vpk>` inspects the compiled models in a VPK (block structure, mesh-part count, embedded geometry, skeleton/physics presence). `model export` writes a `.glb`: choose the source with `--entry <path.vmdl_c>` or `--hero <codename>` (auto-discovers the body model), resolve materials/textures across `--vpk` then `--base <pak01_dir.vpk>`, and write to `--out`. Animation clips are emitted by default; `--clip <name>` (repeatable) exports only the named clip(s) and `--no-anim` drops them. The positional merge invocation and the `split` / `portrait` / `soundevents` subcommands are unchanged.

### Library (`vpkmerge-core` 0.6)

- `export_model`, `export_hero_model`, and `inspect_models` plus `AnimOptions`: open a VPK (and optional base pak), decode a `.vmdl_c` via `morphic`, and write a textured, animated `.glb`, resolving referenced materials/textures across both packages (skin first, base second).

### morphic (0.2)

- New `model` module: a pure-Rust Source 2 `.vmdl_c` decoder and glTF writer. Decodes the skeleton, LOD0 meshes (meshoptimizer vertex/index codecs), per-vertex skin weights, materials (`.vmat_c`) with their `.vtex_c` textures, and the model's own animation clips (`ANIM`/`ASEQ`/`AGRP`: the compressed segment decoders, the 6-byte packed quaternion, half-float vectors), then emits a binary `.glb` (geometry + skin + PBR materials + animation samplers). The KV3 reader gained ZSTD (via `ruzstd`) and the binary-blob section, which the model `ANIM` block uses.

## v0.4.0

Closes the gap between editing a soundevents file and shipping it: the `soundevents` subcommand can now pack its re-encoded output straight into a standalone addon VPK, so an edited (or generated) loose file can finally enter the merge pipeline. This unblocks Grimoire's per-ability volume/pitch path, where a hero's `.vsndevts_c` is decoded, `--set` on the relevant events, and the result shipped in a consolidated addon VPK at the same entry path.

### CLI (`vpkmerge` 0.4)

- New `--encode-vpk OUT_dir.vpk` flag on `soundevents`. After applying `--swap-vsnd` / `--set`, re-encode and pack the file into a standalone single-archive VPK at its entry path, ready to merge. The entry path defaults to INPUT in `--from-vpk` mode; `--vpk-entry PATH` overrides it (and is required for a loose-file input). Combinable with `--encode`; the loose `--encode` and JSON-to-stdout behaviors are unchanged.

### Library (`vpkmerge-core` 0.5)

- New `pack(files, output)`: write in-memory `(entry_path, bytes)` files into a standalone single-archive `_dir.vpk`. The general primitive for getting loose or generated files into a VPK so they can enter `merge`. Creates missing parent directories; produces a chunk-free, engine-loadable addon VPK.

## v0.3.0

vpkmerge grows past pure merging into a small Deadlock asset toolkit: two new CLI subcommands (hero portrait extraction and soundevents editing), a GUI Browse tab that walks a VPK's file tree with live texture previews, and a much-expanded `morphic` that now decodes HDR textures, selects mips/cubemap faces, re-encodes, and reads/writes binary KeyValues3.

### CLI (`vpkmerge` 0.3)

- New `portrait` subcommand. Extract and decode hero card/portrait art from a VPK to PNG. Flags: `--out DIR`, `--hero CODENAME` (omit for every hero), `--manifest FILE` (otherwise the JSON manifest prints to stdout).
- New `soundevents` subcommand. Decode a `.vsndevts_c` file to JSON, edit it, and re-emit a loadable file. Reads a file on disk or, with `--from-vpk VPK`, an entry path inside a VPK. Edits: `--swap-vsnd OLD=NEW` (rewrite a clip path everywhere), `--set EVENT/FIELD=NUMBER` (set a numeric field like volume/pitch). `--encode OUT` writes an uncompressed v4 file the engine can load; without it the decoded tree prints as JSON.
- The positional merge invocation and the `split` subcommand are unchanged.

### Library (`vpkmerge-core` 0.4)

- `extract_portraits(vpk, hero, out_dir)` plus `PortraitInfo` and `PortraitVariant`: locate hero portrait textures in a VPK and decode them to PNG (via `morphic`), reporting per-texture format/size and skip reasons.
- `SoundEvents` (load `from_file` / `from_vpk` / `from_bytes`, `to_json`, `swap_vsnd`, `set_event_field`, `encode`, `original_len`) plus `EventSummary`: the soundevents-aware layer over `morphic`'s KV3 codec, built for a per-ability sound picker (control volume/pitch/clip choice, not just swap audio).

### GUI (0.3)

- **Browse tab.** New top-level tab alongside Merge. Auto-loads the local Deadlock `pak01_dir.vpk`, walks the VPK file tree with a filter, and renders a live preview of any selected `.vtex_c` via `morphic`.
- **Collision default flipped to top-of-list-wins.** The highest-priority (top) mod now overrides by default, matching the visible priority order. Still exposes Bottom wins and Refuse.
- **HDR previews tone-mapped** instead of erroring out, so BC6H / float textures show a sensible thumbnail.
- Long entry paths in the Browse tab scroll horizontally instead of overflowing.

### `morphic` 0.1.0

- **BC6H decode** (HDR, `Rgba16F` output) with an HDR golden path: LDR formats diff against a PNG, HDR formats against a raw `.f32` sibling with per-channel tolerance.
- **Encoders + splicing.** `encode_image` (BCn, BC6H, RGBA8, inline PNG passthrough) plus splice entry points: `replace_face0_mip0` / `replace_face_mip` (single mip, rest byte-exact) and `replace_mip_chain` / `replace_face_mip_chain` (regenerate the full mip pyramid from a new mip 0).
- **Mip + cubemap face selection.** `DecodeOptions { mip, slice, face }` decodes any mip level or cubemap face.
- **Binary KeyValues3 codec** (`morphic::kv3`): reads v1..=5 (including v5 two-buffer / LZ4), writes v4 uncompressed. `decode_kv3_resource` / `encode_kv3_resource` wrap the resource envelope, preserving the format GUID and `RED2` block on re-encode. This is what powers `SoundEvents`.
- Format coverage is now roughly 86% of Deadlock's `.vtex_c` corpus by count. RGBA16161616F, inline WebP, 3D depth slices, and texture arrays remain pending (non-blocking in the golden harness).

### Packaging / release

- **AUR packages** under `packaging/aur/`: `vpkmerge-bin` (GUI from the release `.deb`), `vpkmerge-cli-bin` (raw CLI binary), and `vpkmerge-git` (both, built from HEAD).
- Portable Windows `.exe` added to the release artifacts (no install; needs WebView2).
- Release notes now lead with a per-platform Download table for both the desktop app and the CLI.

## v0.2.0

The first release with both directions of the operation: combine many VPKs into one, or split one into many. The GUI gets a substantial visual overhaul and the conflict resolver gains texture-aware previews.

### Library (`vpkmerge-core` 0.3)

- New `split` API symmetric to `merge`: route entries from one input VPK into N outputs by path predicate, with optional residual bucket and three overlap policies (`FirstMatch`, `AllMatches`, `Error`).
- New types: `SplitOutput`, `PathPredicate::AnyPrefix`, `SplitOptions`, `SplitReport`, `SplitOutputReport`.

### CLI (`vpkmerge` 0.2)

- New `split` subcommand. Takes a JSON plan file describing outputs and prefix predicates; flags: `--strict`, `--all-matches`, `--residual`, `--verbose`. See [`docs/splicing.md`](./docs/splicing.md).
- Existing positional merge invocation continues to work unchanged.

### GUI (0.2)

- **Settings**: gear in the title bar opens a Settings panel.
  - Theme: Light / Dark / System (follows OS in real time).
  - Doodles: four backgrounds (arcane, celestial, botanical, nautical) in light and dark variants.
  - Candlelight: optional warm corner glow with a slow flicker and sway.
  - Diagnostics: export the current session log to a text file.
- **Texture preview** in the conflict resolver: `.vtex_c` collisions now show thumbnails of each candidate's texture so you can pick the winner by sight. Powered by the new `morphic` decoder.
- **Layout**: the bottom bar is gone. Merge is now a paper-card section at the end of the form, with a live summary, the conflicts pill, and a prominent Merge button (wax-seal shadow treatment so it doesn't blend with the radio buttons above).
- **Merged modal**: animated checkmark on the success header.
- **Merge with no output set**: clicking Merge now opens the save picker instead of erroring.

### New crate (`morphic` 0.0.1)

Pure-Rust Source 2 `.vtex_c` decoder. Format support: BC7, DXT1, DXT5, ATI1N, ATI2N, RGBA8888, and the inline PNG path. A committed golden corpus diffs each decode against a ValveResourceFormat-generated reference. Not on crates.io yet.

### Repo

- `morphic-oracle` C# dev harness under `tools/morphic-oracle/` regenerates the golden corpus. CI never runs `dotnet`.
- `Justfile` orchestrates the daily loop (`just`, `just ci`, `just survey`, `just fixture`).
- Cross-OS release workflow at `.github/workflows/release.yml` ships GUI bundles and CLI binaries for Linux, macOS (x86_64 + aarch64), and Windows on every `v*.*.*` tag.

## v0.1.x

Earlier releases. See `git log` for details.
