# Hero skeleton donor map (which heroes a model swap can drop onto)

We shipped one custom-hero model swap (the gen_man "hat-man" ghost onto **Wraith**) and
proved it loads + animates in-game. Wraith worked because gen_man and Wraith share the
*exact same* core rig. This doc answers the obvious follow-up: **which other heroes share
that bone structure, so the same swap drops on with little or no rebind?**

TL;DR: the roster is effectively **one humanoid rig family**, but every hero is authored in
a slightly different *rest pose*. Only **Wraith, Sinclair (`magician`), and Mirage (`mirage`)**
share gen_man's pose exactly: they are zero-rebind drop-in donors. Everyone else needs the
inferno-style rebind (rebind the mesh to *their* pose + a small finger/IK weight remap), and
a long tail is genuinely hard (very different pose or a reduced rig).

## How this was measured

Tool: `vpkmerge-core/examples/hero_skeleton_matrix.rs`

```
cargo run --release -p vpkmerge-core --example hero_skeleton_matrix -- <pak01_dir.vpk> [--ref ENTRY] [--json]
```

It reads every live hero's skeleton (from `heroes.vdata` `m_strModelName`), decodes the
compiled `m_modelSkeleton`, and compares each against the gen_man ghost rig
(`models/npc/ghosts_active/gen_man_ghost.vmdl_c`, 116 bones).

**The metric that matters (and the trap).** A naive compare of the stored *parent-relative*
bind rotations is misleading:

- gen_man parents `pelvis` at the root, but most heroes parent it under `root_motion` (whose
  own bind is itself a ~120deg rotation). So a parent-relative compare shows a phantom 120deg
  on the pelvis even when the *world* pose is identical.
- IK targets (`*_IKTARGET`), leaf tips (`*_end*`), and attachment bones (`object_hand_*`,
  `weapon_hand_*`) carry **no skin weight**, so their bind orientation is irrelevant to a mesh
  swap.

The honest signal is **world-space bind rotation over deformation bones only**, plus the count
of **missing deformation bones** (gen_man deform bones the target lacks -> undriven verts). The
tool computes both. It reproduces the two empirical points we already had exactly (Wraith
~0.2deg, Inferno ~24deg / 33 missing), so the numbers are trustworthy.

Columns below: `shar` = shared bone names / 116, `mDef` = missing **deform** bones,
`wRotMean/Max` = world-space deform-bone bind-rotation delta (deg), `ht` = skeleton
pelvis->head height (gen_man ref = 33.6; the scale-calibration hint).

## Tiers

### Drop-in donors (zero rebind: reuse gen_man's mesh + weights verbatim)

| code | display | bones | mDef | wRotMean | ht | note |
|---|---|---|---|---|---|---|
| `wraith` | Wraith | 425 | 0 | 0.24deg | 32.4 | shipped + in-game confirmed |
| `magician` | Sinclair | 179 | 3 | 0.00deg | 33.6 | missing only jaw_0/eye_L/eye_R (face) |
| `mirage` | Mirage | 219 | 3 | 0.00deg | 33.6 | missing only jaw_0/eye_L/eye_R (face) |

Sinclair and Mirage are the same clean core rig as Wraith. Their only deform-bone gap is the
jaw + eyes, which a fedora mannequin mesh (or any mesh without separate eye/jaw geometry) does
not weight to. They are genuine zero-rebind donors we had not used.

### Body-match, finger-only divergence (drop-in for a fingerless mesh)

| code | display | mDef | wRotMean | worst bones |
|---|---|---|---|---|
| `doorman` | - | 6 | 6.5deg | only the pinky chain (17deg) |
| `fencer` | Apollo | 6 | 9.0deg | only the thumb chain (15-19deg) |

Arms/legs/spine/head match gen_man; the *only* difference is finger curl. For the hat-man
(molded hands) these are effectively drop-in too.

### Remap (inferno-class: rebind to their pose + small finger/IK weight remap)

~13 heroes, world-space deform delta roughly 13-25deg, few missing deform bones:
`viscous`, `astro` (Holliday), `pocket`, `nano` (Calico), `punkgoat`, `bookworm` (Paige),
`necro` (Graves), `abrams`, `familiar` (Rem), `unicorn` (Celeste), `warden`, `vampirebat`
(Mina), `inferno` (Infernus). This is the path we already proved twice (Infernus, Rem).

### Hard (very different rest pose, or a reduced/different rig)

~22 heroes: `hornet` (Vindicta, a tiny 62-bone rig missing 46 deform bones), `haze` (107deg),
`digger`, `geist`, `chrono` (Paradox), `yamato`, `mcginnis`, `archer`, `bebop`, `viper`, `ivy`,
`kelvin`, `shiv`, `drifter`, `werewolf` (Silver), `lash`, `dynamo`, `wrecker`, `priest`,
`operative`, `gigawatt_prisoner`, `frank` (Victor). Doable but it is a real rebind + weight-
authoring job, and Vindicta-class reduced rigs may not be worth it.

### Not shipped in current pak01

`boho`, `druid`, `fortuna`, `graffiti_girl` are unreleased WIP heroes: `heroes.vdata` has the
record but the model file is not in the pak yet.

## The universal 24-bone core

These 24 bones are present in **every** hero, so a mesh weighted to only these swaps onto
anyone with no missing-bone gaps:

```
root_motion, pelvis, spine_0, neck_0, head,
clavicle_L/R, arm_upper_L/R, arm_lower_L/R, hand_L/R,
leg_upper_L/R, leg_lower_L/R, ankle_L/R, ball_L/R,
finger_thumb_0_R, finger_middle_0_R, finger_middle_1_R
```

It excludes `spine_1/2/3`, all twist bones, all IK, eyes/jaw, and most fingers. Enough for a
blocky / rigid custom character (the BMO case), not a full deforming humanoid.

## Practical guidance

1. **Pick a donor by `mDef` first, then `wRotMean`.** Zero missing deform bones + ~0deg = reuse
   gen_man's bind verbatim (Wraith/Sinclair/Mirage). Higher `wRotMean` means you must rebind the
   mesh to *that hero's* rest pose in Blender (the build pipeline does this when you feed it the
   donor skeleton); it is not a blocker, just work.
2. **Height still needs calibration even on a drop-in.** `ht` varies (gen_man 33.6, abrams/viper
   41, several WIP ~24). Scale the FBX so the compiled mesh's bbox-height / injected pelvis->head
   matches gen_man's native ratio (2.927); see the scale notes in
   `exports/gen-man-hero/REBUILD_hatman_wraith.sh`.
3. **This measures rig compatibility only.** A real swap still needs the CSDK compile + NM-ref
   injection + in-game test (see [handoff-vertex-color-recolor] and the gen-man-hero build
   scripts). Rig compatibility just tells you how much rebind work to expect.

## Related

- `exports/gen-man-hero/REBUILD_hatman_wraith.sh` - the proven drop-in build (Wraith).
- `tools/hero-model-compiler/build_hero_model.py` - the rig -> compile -> inject -> pack pipeline.
- Memory: `hero-skeleton-donor-map`, `genman-wraith-shared-skeleton`, `hat-man-infernus-model-swap`.
