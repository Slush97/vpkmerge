# Spike: headless Mixamo retargeting onto Deadlock heroes

Goal: drop an arbitrary Mixamo (or other humanoid) `.glb`/`.fbx` animation onto a
hero clip slot with no Blender and no Rokoko, closing the last manual step in the
`gltf_import` authoring loop.

Prior art is Pugzilla's "Deadlock - Simple Animation Changes" (2026-07-25), which
does this by hand: Source 2 Viewer decompile -> Blender -> Rokoko retarget -> hand-wire
6 IK constraints -> Blender Source Tools DMX export -> repoint `source_filename` in the
uncompiled `.vnmclip` -> CSDK recompile -> pack. Everything below replaces the middle
three steps.

## Status

Measurement done against live `pak01` (2026-07-28). Probes:
`vpkmerge-core/examples/retarget_probe.rs`, `vpkmerge-core/examples/retarget_bones.rs`.

**Work item 1 (`read_glb_skeleton`) is landed and validated.** Everything else below
is still design. The source rest pose is now readable, so the retarget math is
unblocked.

## What we measured

`haze` (96 bones) and `wraith` (427 bones) out of
`/mnt/storage/SteamLibrary/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk`.

### The core chain is stable and small

```
root_motion                       <- root #1 (motion track + IK targets)
  leg_L_OFFSET -> leg_L_IKTARGET
  leg_R_OFFSET -> leg_R_IKTARGET
  arm_L_IKTARGET
  arm_R_IKTARGET
pelvis                            <- root #2 (the actual body)
  spine_0 -> spine_1 -> spine_2 -> spine_3
    neck_0 -> head
    clavicle_L -> arm_upper_L -> arm_lower_L -> hand_L -> weapon_hand_L
    clavicle_R -> arm_upper_R -> arm_lower_R -> hand_R -> weapon_hand_R
  leg_upper_L -> leg_lower_L -> ankle_L -> ball_L
  leg_upper_R -> leg_lower_R -> ankle_R -> ball_R
```

Two roots, not one: `root_motion` and `pelvis` are both parentless. The IK bones hang
off `root_motion`, not off the limb they drive.

Of haze's 96 bones, 38 are fingers and only ~29 are core locomotion. haze and wraith
share those 29. The rest is per-hero accessory clutter (wraith carries ~300
`$cloth_m0p*` FeModel cloth nodes, coat/sleeve/ponytail chains, `inner_*` twist helpers).

Naming drift between heroes exists but is confined to accessories: haze has
`weapon_hand_L`, wraith has `weaponHand_L`; wraith adds `attachHand_L/R`. **The core 22
do not drift.** Unmapped bones already do the right thing in `apply_animation` (they
keep the slot's original channels), so accessory drift costs nothing.

### The bind pose is arms-down, and that is the whole problem

Angle off horizontal, measured bone-origin to first-child, Source Z-up (+X forward,
+Y left, +Z up):

| bone | direction | angle off horizontal |
|---|---|---|
| `clavicle_L` | (-0.21, 0.82, -0.53) | +31.8 deg |
| `arm_upper_L` | (-0.04, 0.32, -0.95) | **+71.5 deg** |
| `arm_lower_L` | ( 0.20, 0.31, -0.93) | **+68.4 deg** |
| `hand_L` | ( 0.33, -0.18, -0.93) | +68.1 deg |
| `leg_upper_L` | ( 0.09, 0.16, -0.98) | +79.5 deg |
| `leg_lower_L` | (-0.12, 0.13, -0.98) | +79.7 deg |
| `ankle_L` | ( 0.80, 0.12, -0.58) | +35.7 deg |

Arms hang at ~71 deg below horizontal. That is neither a T-pose (0 deg) nor a
conventional A-pose (~45 deg). Mixamo authors in a strict T-pose, so **the per-bone
rest correction on the arm chain is on the order of 70 degrees.**

This settles the design question: copying Mixamo's local rotations across without a
rest-pose correction is not "slightly off," it puts the arms 70 degrees wrong on
every frame of every clip. The correction is mandatory, not a refinement.

### Scale

- Bone z-extent: 0.00 .. 83.60 (span 83.60)
- `pelvis` height: **53.91**

Pugzilla's Blender step uses 39.37 (inches per metre) for unit conversion, then
eyeballs a second scale-up to fit the hero. The measured hip ratio is why: a
metre-scale Mixamo rig converted at 39.37 lands its hips at 39.37 against Deadlock's
53.91, about 27% short. A headless retargeter should derive the factor from the hip
height ratio rather than eyeballing it.

### IK bone presence is per-hero

haze has all six (`arm_L/R_IKTARGET`, `leg_L/R_OFFSET`, `leg_L/R_IKTARGET`). wraith has
the four leg ones but **no arm IK targets**. So IK driving must be detected per rig,
never assumed. This matches the video's aside that Haze in particular needs its IK
aligned or the animation breaks.

## Why not Rokoko

Rokoko fuzzy-matches because it must accept arbitrary user rigs. Both rigs here are
fixed, so the mapping is a table written once. A crude fuzzy matcher was tried as a
control (`retarget_probe.rs`) and scored 11/22 with actively wrong hits: it matched
`LeftForeArm` to `forearm_tie_0_L`, a **cloth tie bone**, rather than `arm_lower_L`.
Fuzzy matching is not merely unnecessary here, it is a hazard. A hand table is 22/22.

## Proposed mapping table

| Mixamo | Deadlock |
|---|---|
| `Hips` | `pelvis` |
| `Spine` / `Spine1` / `Spine2` | `spine_0` / `spine_1` / `spine_2` |
| `Neck` | `neck_0` |
| `Head` | `head` |
| `LeftShoulder` / `RightShoulder` | `clavicle_L` / `clavicle_R` |
| `LeftArm` / `RightArm` | `arm_upper_L` / `arm_upper_R` |
| `LeftForeArm` / `RightForeArm` | `arm_lower_L` / `arm_lower_R` |
| `LeftHand` / `RightHand` | `hand_L` / `hand_R` |
| `LeftUpLeg` / `RightUpLeg` | `leg_upper_L` / `leg_upper_R` |
| `LeftLeg` / `RightLeg` | `leg_lower_L` / `leg_lower_R` |
| `LeftFoot` / `RightFoot` | `ankle_L` / `ankle_R` |
| `LeftToeBase` / `RightToeBase` | `ball_L` / `ball_R` |

One wrinkle: Mixamo has 3 spine bones, Deadlock has 4. Leave `spine_3` static
(inherits its parent, visually negligible) rather than redistributing rotation across
four bones. Fingers map 1:1 by the same pattern if wanted, but are lower value.

## Design

### The math

Rotation-only retarget with a constant per-bone rest correction:

```
C(b)     = Bs(b)^-1 * Bt(b)                 // source/target global bind, precomputed once
At(b,f)  = As(b,f) * C(b)                   // retargeted model-space rotation
Lt(b,f)  = At(parent(b),f)^-1 * At(b,f)     // back to parent-local, what NmTrack stores
```

`Bt` comes from `Bone::global_bind` (`morphic/src/model/skeleton.rs:40`), already
available. `Bs` is the gap: `read_glb_animation` reads animation channels only and
discards the source node hierarchy and rest transforms.

Root translation is the one non-rotation channel worth carrying: scale it by
`target_hip_z / source_hip_z` and apply only where the slot already animates root
translation.

### Work items

1. ~~**`read_glb_skeleton(glb)`**~~ **DONE.** `morphic::model::read_glb_skeleton` returns
   a `GltfSkeleton { bones: Vec<GltfBone>, bind_source }`, each bone carrying name,
   nearest-joint-ancestor parent, rest TRS, `local_bind`, and `global_bind`.
   `global_bind` prefers the skin's `inverseBindMatrices` (glTF's authoritative bind
   pose) and falls back to chained rest TRS, reported via `BindSource`. Parents resolve
   by walking the whole node tree, so FBX-converted rigs that interleave non-joint
   nodes still chain correctly.

   Validated three ways:
   - `glb_skeleton_rest_pose_round_trip` (`tests/gltf_anim_roundtrip.rs`): synthetic
     arms-down rig through the real `to_glb`, asserting names, parents, rest TRS, and
     `global_bind`.
   - Unit tests for out-of-hierarchy-order joints and cycle rejection.
   - **Real-rig check** (`vpkmerge-core/examples/retarget_glb_skel.rs`): Haze's 96-bone
     rig exported to `.glb` and read back against pak-derived truth. 0 name mismatches,
     0 parent mismatches, worst bone-origin error 1.2e-5 units across an 83.6-unit rig
     (f32 noise). It independently reproduces the +71.5 deg `arm_upper_L` figure that
     was measured straight from the KV3, so the two paths agree.

   This also confirms the convention claim the reader rests on: glTF's column-major
   matrix and morphic's row-vector `Mat4` are transposes, so the flat float sequence is
   identical and inverse-bind matrices round-trip with no reordering.
2. **`BoneMap`**: the static table above, plus an override hook so an odd rig can be
   remapped without a code change.
3. **`retarget(clip, nm_skel, model_skel, src_skel, anim, map) -> NmClip`**: the math
   above, sitting beside `apply_animation` rather than replacing it. Signature mirrors
   `nm_clip_to_clip(clip, nm_skel, model_skeleton, name)`, which already takes both the
   names-only `NmSkeleton` (for track order) and the bind-bearing `Skeleton`, so the
   plumbing precedent exists.
4. **IK driving**: after FK retarget, write each present `*_IKTARGET` bone from the
   model-space transform of the limb end it tracks (`arm_*_IKTARGET` <- `hand_*`,
   `leg_*_IKTARGET` <- `ankle_*`, `leg_*_OFFSET` <- `leg_lower_*`). Detect presence per
   rig; skip silently when absent. This is the mechanical equivalent of the video's six
   hand-wired Copy Location constraints, and the step most likely to be done wrong by
   hand.
5. **CLI**: `vpkmerge model retarget --vpk <VPK> --hero <CODE> --clip <SLOT> --glb
   <FILE> [--fbx <FILE>] --encode-vpk <OUT_dir.vpk>`.

### Validation, before anything goes in-game

- Null round-trip: retarget a hero's own exported clip back onto itself through the
  full path; per-bone angle diff should be ~0. `examples/anim_glb_diff.rs` already does
  this measurement for the import path.
- Bind-delta sanity: assert the computed `C(b)` for `arm_upper_L` is ~70 deg against a
  T-posed source. If it comes out near 0, the source rest pose was not read.
- Visual: `nm_clip_preview_glb.rs` for an offline eyeball before packing.

## Known ceiling

`apply_animation` time-stretches onto the slot's fixed frame count, and
`reencode_nm_clip` is an equal-length in-place patch. Haze's ult is 34 frames, roughly
a second; a 3-second Mixamo clip squeezed in plays ~2.6x fast and undersampled.

Pugzilla's route dodges this entirely by never touching the pose blob: he repoints
`source_filename` and lets `resourcecompiler` regenerate it at whatever length. So his
quality ceiling is genuinely higher than ours until the v5 clip encoder in
[anim-authoring-pipeline.md](anim-authoring-pipeline.md) lands.

Mitigation short of that: pick a slot whose frame count roughly matches the source, and
trim rather than stretch. Retargeting is still worth building first, since the encoder
does not remove the need for it.

## Caveat on these numbers

The **target** side is measured from live pak01 and is solid. The **source** side is
not: no Mixamo file was available (Mixamo requires an Adobe login), so the T-pose /
Y-up / ~1m-hip conventions are taken from Mixamo's documented rig, not measured. The
~70 deg correction figure assumes a strict T-pose source. First build step should
measure a real Mixamo export and confirm before trusting the constant.
