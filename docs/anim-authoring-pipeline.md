# Custom animation authoring pipeline (end-state design)

Design note for the eventual "hand-author a Deadlock animation in Blender and play
it in-game" workflow. Captures where we are, the end-state UX, the engineering
still missing, and the constraints. Context: the quantized-pose codec for
`.vnmclip_c` is done and **in-game confirmed** (see
[handoff-nm-loose-clip-pose.md](handoff-nm-loose-clip-pose.md) and the NM section
of [../CLAUDE.md](../CLAUDE.md)); everything so far edits *existing* compiled clips
programmatically. This doc is the plan for true hand-authoring.

## Two layers

1. **Authoring (artist tools).** Blender or Maya: pose the rig, keyframe, IK, see
   the mesh deform. Produces a *source* animation.
2. **Compiled engine files.** Source 2's compiler (ModelDoc / resourcecompiler)
   turns that source into the compiled `.vnmclip_c` / `.vnmgraph_c`. This is the
   layer morphic reads and writes.

All work to date lives in layer 2 (decode the compiled quantized stream, transform
it, re-encode). Great for *derived* edits (amplify, spin, overlay, retime); not how
you key a dance from scratch, and limited to channels the clip already animates.

## The official pipeline (informational)

```
Blender/Maya -> FBX or DMX -> Source 2 ModelDoc -> resourcecompiler -> .vnmclip_c + graph
```

Deadlock has **no official modding/animation SDK released** (as of this work), so
this path isn't available to mods. And even with it, the animation graph resolves
clips by **compile-baked references** — late-added files at new paths are ignored
(proven; see the handoff doc's round 4). So a new clip can only enter the game by
**replacing an existing referenced clip at its path** — the same override mechanism
we already use. The SDK would only change *how the bytes are authored*, not *how
they get into the game*.

## End-state workflow (SDK-free, the target)

```
Blender (key against the exported skeleton)
  -> glTF
  -> morphic importer (sample per-frame per-bone, map by bone name)
  -> NmClip -> encode_compressed_pose
  -> write the compressed-pose section into the .vnmclip_c
  -> pack at the slot's path -> addon VPK
```

Artist's view, three commands:

1. **Export the rig to author against** (already built):
   ```
   vpkmerge anim export --hero yamato --slot reload_idle --out yamato_reload.glb
   ```
   A skinned, animated GLB: real mesh, real skeleton, the existing clip as
   reference. (`morphic::model::{decode, nm_clip_to_clip, to_glb}` +
   `examples/nm_clip_preview_glb.rs` do this today.)

2. **Animate in Blender.** Keyframe the armature. One rule: do not rename or
   reorder bones (mapping is by bone name). Export glTF with the animation.

3. **Import + pack** (built 2026-06-14, as `examples/nm_clip_import_glb.rs`):
   ```
   cargo run --release -p vpkmerge-core --example nm_clip_import_glb -- \
       pak01_dir.vpk \
       models/heroes_wip/yamato/clips/reload_idle_quick.vnmclip_c my_taunt.glb \
       taunt_dir.vpk \
       models/heroes_wip/yamato/clips/reload_idle.vnmclip_c \
       --mesh models/heroes_staging/yamato_v2/yamato.vmdl_c --preview taunt_preview.glb
   ```
   Reads the slot's clip + its `.vnmskel_c`, imports the glTF animation onto it
   (`morphic::model::import_glb_onto_nm_clip`, v5 in-place), and packs the result at
   the slot path(s) into an addon VPK. The optional `--mesh`/`--preview` writes a
   playable GLB to eyeball the imported motion first. (A first-class `vpkmerge anim
   import` CLI subcommand is the eventual Grimoire-facing wrapper around the same
   core call.)

Then install in `game/citadel/addons/` (or let Grimoire number it) and trigger the
slot in-game.

## Built vs. missing

**Built and proven:** the quantizer (`encode_compressed_pose`), the byte-faithful
blob splice (`patch_kv3_resource_blob`), glTF export + preview (`nm_clip_to_clip`,
`to_glb`), and in-game acceptance of re-encoded clips (single-hue, bow, spin,
moonwalk, slow-mo all confirmed). The codec round-trips (decode -> encode -> decode
pose-identical, 90%+ byte-exact).

**Arbitrary-length blob write — DONE (2026-06-14), multi-frame DONE (2026-06-14).**
`replace_blob_v5` (via `kv3::set_sole_blob` / `morphic::patch_kv3_resource_sole_blob`)
writes a pose blob of **any length**: it chunks the blob into 16 KB LZ4 frames
(each an independent block; the dict-aware reader decodes them), grows the per-frame
size table, and updates `sizeBlobs`/`comp2`/`comp_total`/`sizeBlockCompressed` and
`unc2` (the table lives in buf2's tail, so a changed frame count changes buf2's
uncompressed size). This removed the earlier single-frame (~16 KB) cap, so **long
clips edit in place**: `reload_idle` (61f), `sleep_idle` (121f), and the stand idles
were previously rejected ("exceeds one LZ4 frame") and now re-encode fine. CI:
`sole_blob_resize_round_trips` (single-frame) and `sole_blob_multi_frame_round_trips`
(~40 KB / 3 frames, byte-exact round-trip). In-game confirmed via the 6-7 gesture
authored onto Yamato's reload + sleep slots and Haze's weapon-stand + sleep slots
(multi-frame blobs). `set_scalars` and `set_bools` gained the blobbed-LZ4-v5 branch
(decompress-working + reassemble) they were missing, so offsets and flags can be
patched on a clip.

**Encoder v1 — DONE, IN-GAME CONFIRMED (2026-06-14).**
`morphic::model::reencode_nm_clip(original, &NmClip)` re-encodes a clip with a
**changed animated-rotation channel set at a fixed frame count**: rotation tracks
may be added (a static bone becomes animated) and re-posed; it splices the (now
longer/shorter) stream, rewrites the per-frame offsets, and flips each
newly-animated bone's `m_bIsRotationStatic`. CI: `reencode_adds_a_rotation_channel`.
Verified in-engine: a re-encode that converted all 68 static rotation tracks of
Yamato's `reload_idle_quick` to animated loaded and played (whole upper body
wobbled). The container fix that unblocked it: a blobbed KV3 block has a second
`0xFFEEDD00` trailer after the compressed blob frames that the engine asserts on
(morphic's reader ignores it); `replace_blob_v5` now re-appends it. Validated
offline against VRF/Source2Viewer (the engine-grade KV3 reader) before in-game.

**Bone masks are a real constraint.** Only the **upper body** moved in that test:
the animgraph blends `reload_idle_quick` over an upper-body bone mask, so authored
leg/lower-body channels are discarded for that slot even though the clip carries
them. Pick the slot by which bones it un-masks: a full-body idle (e.g.
`hideout_stand_idle`) for whole-body motion, an upper-body slot for arm/torso work.

**IN-GAME RESULT (2026-06-14): the v4 full re-encode is ENGINE-INERT.** A clip
re-encoded via `encode_kv3_resource` (uncompressed v4) loads with no KV3 error and
opens in Source2Viewer, but the engine's NM animation runtime does **not** read its
pose data -- it does not animate, even for channels the clip already animates
(tested: a v4-re-encoded reload that animated via the v5 path showed nothing; a
from-scratch idle showed nothing). NM clips ship v5; downgrading to v4 makes the
pose blob invisible to the animation system. **Only the v5 byte-faithful in-place
path animates in-game** (`patch_kv3_resource_blob` / `set_*`, LZ4 preserved -- the
bow/spin/moonwalk/slow-mo effects). So `reencode_nm_clip_full` is an **offline-only**
tool (round-trip tests, GLB preview, S2V) until morphic gains a **v5 KV3 writer**;
the gaps it "closes" (translation/scale adds, frame-count change, from-scratch) are
NOT yet achievable in-game. To deliver them in-game: write a v5 encoder (two-buffer
+ LZ4 frames) so a full re-encode stays v5, or do the structural edits surgically in
the v5 container. Separately, whether the engine even reads *added* channels (vs a
compile-baked channel set) is still open -- the reload add test was confounded by
the slot's upper-body bone mask.

**Full encoder (offline only) -- DONE (CI; NOT engine-viable, see above).**
`morphic::model::reencode_nm_clip_full(original, &NmClip)` closes both remaining
gaps by **rebuilding the whole DATA block from the value tree** (uncompressed v4 via
`encode_kv3_resource`) instead of patching in place. Each former blocker becomes a
plain tree edit: adding a **translation/scale channel** (its range is recomputed
from the channel's min/max and written as a real value -- the writer re-tags
everything, so no tagless-constant problem) and changing the **frame count** (the
`m_compressedPoseOffsets` array is rebuilt and `m_nNumFrames` set). It recomputes
every animated channel's range from data, so untouched channels re-quantize within a
sub-step (use the in-place `reencode_nm_clip` when byte-faithfulness matters). CI:
`full_reencode_adds_a_translation_channel`, `full_reencode_changes_frame_count`,
`full_v4_reencode_round_trips`. The identity v4 re-encode opens cleanly in
Source2Viewer (VRF); an *edited* v4 re-encode still needs an in-game confirmation
(v4-uncompressed clips are new in-engine territory; the v5 surgical path is already
confirmed).

So the encoder is feature-complete for the importer:
- **Resampling to the slot's clock** is now free either way -- match the frame count
  (in-place) or change it (full re-encode).
- **Engine timing** is clip-duration-driven (confirmed via the Warden slow-mo test),
  so a resampled clip plays at the slot's duration.

**Update (2026-09-29): v5 writer landed; test (a) IN-GAME CONFIRMED.** A rotation edit rebuilt whole through the v5 path animates in the live engine, so the earlier inert result came from the lossy v4 writer, not from rebuilding the clip. (b) frame-count change and (c2) added translation channels (neck +20, clavicles +15 on bones the slot kept translation-static) also confirmed. With that, the in-place limits below no longer bind: `nm_clip_import_glb --full` takes any rotation/translation/scale edit at any frame count. Still true: replace-only slots, same skeleton, bone masks, cloth bones. The v4 output was
built from the folded `Value` tree, which also dropped Valve's typed-array framing
(`m_compressedPoseOffsets` is an auxiliary-buffer `UINT32` array,
`m_trackCompressionSettings` a typed `OBJECT` array) and value flags. The same loss is
what turned an untyped model re-encode into an error model, so it is at least as
likely a cause of the inert clip as v4 itself. `morphic::kv3::encode_v5` (lossless,
byte-identical buffers against all of pak01) plus `encode_v5_like` now back
`reencode_nm_clip_full`, and `import_glb_onto_nm_clip_full` adds translation/scale
channels and frame-count resampling on top. Test addons in `exports/anim-v5-tests/`:
(a) rotation-only through the full path, (b) 21 -> 42 frames, (c) a head translation
channel added. VRF parses all three as `AnimationClip`.

## De-risking order

The full clip encoder landed first (`reencode_nm_clip` in-place + the offline
`reencode_nm_clip_full`), proven byte-/pose-faithfully on the committed fixtures and
across a full pak (`tests/nm_clip.rs`, `tests/nm_clip_local.rs`).

**Blender importer -- DONE (2026-06-14).** `morphic::model::gltf_import` is the thin
layer the encoder made possible: read a `.glb`'s animation back into per-bone-name
TRS tracks (`read_glb_animation`, on the `gltf` reader crate), time-stretch them onto
the slot's frame grid mapping bones by name (`apply_animation`), and splice via the
v5 in-place `reencode_nm_clip` (`import_glb_onto_nm_clip`). It honors the in-place
limits: rotations may be edited or **added**, translation/scale are edited only where
the slot already animates them. CI (`tests/gltf_anim_roundtrip.rs`):
`glb_writer_reader_round_trip` proves the writer/reader contract (author a skeleton +
clip, `to_glb`, read it back, every TRS channel survives keyed by bone name), and
`apply_animation_maps_by_name_and_respects_constraints` proves the map-and-edit logic
against a real fixture clip. End-to-end pack: `examples/nm_clip_import_glb.rs`.

**IN-GAME CONFIRMED (2026-06-14): the full Blender authoring loop works.** A torso
swivel hand-keyed on Yamato's rig in Blender (via blender-mcp), exported to glTF,
imported with `nm_clip_import_glb`, and packed at the reload slot loaded and played in
the live engine. The decisive de-risk first: a **null round-trip** (import our
reference GLB into Blender, re-export unchanged, run it back through `apply_animation`)
measured **0.51 deg mean** per-bone rotation difference versus the original clip,
proving Blender's glTF import/export preserves the per-bone coordinate frame our
importer reads (so authored rotations map onto the game rig without a correction). Two
helper examples support the loop: `gen_obvious_anim_glb.rs` (synthesize an exaggerated
animation as a real `.glb` with no Blender, for testing the read path) and
`anim_glb_diff.rs` (the null-round-trip diff tool: per-bone angle difference between a
clip and a glb after import-mapping).

## Grimoire integration (eventual)

Folds into the mod manager: pick a hero + slot, drop a `.glb`, and it shows the clip
**animating on the actual hero mesh in-browser** (three.js — why the GLB export and
the IBL/cubemap work matter) before committing; a cheap spring-sim on the cloth/hair
bones papers over the runtime-sim gap so the preview reads right; one click installs.
Same morphic core behind the CLI and Grimoire. Trust story: author visually, preview
on the real mesh, and the round-trip guarantee means what you preview is what ships.

## Constraints (hold regardless of tool)

- **Replace-only** — overwrite an existing slot; cannot register a new animation.
- **Same skeleton** — no cross-hero retarget without a bone retargeter (heroes have
  unique rigs, 98-126 bones).
- **Match the slot's additive type** — additive deltas and absolute poses are not
  interchangeable.
- **Cloth/hair bones are runtime-simulated**, never in clips, so they won't follow an
  authored pose (preview dodge: spring-sim them in three.js).
- **Online**: the server animates vanilla, so own-hero / cosmetic surfaces only.
