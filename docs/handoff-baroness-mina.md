# Baroness Mina: combined-mod import, skeleton artifacts, and the bald-spot saga

Status (2026-07-03): v4 in-game confirmed; v5 (PHYS splice) WARPED the ears
in-game (block unregistered, see below); v6 (PHYS registered in CTRL) still
MISPOSITIONED the ears + keys in-game; v7 (anchor bones unflagged) changed
nothing (the donor FeModel turned out to be structurally dead, see "v8"
below); v8 (whole ear chain unflagged) reported pixel-identical to v7 (user
restart unconfirmed); **v10 (ears WELDED to `head` + purse/keys assembly
rigid-pinned to `spine_2`) installed, offline-verified in posed renders,
in-game reconfirm pending.** v9 = the ear weld alone (`weld_mina_ears.rs`,
199 verts, the ears-only subset of `fix_mina_artifacts` whose duster
assertion refuses to re-run on this lineage). v10 adds the purse pin via the
new generic `reskin_bones_to.rs` (16 bones -> `spine_2`, 5902 verts: bag on
`purse_0/purse_end`, keys on the `gun_key_*` chains). Why: the vanilla NM
runtime skeleton contains and ANIMATES `purse_0..3` / `purse_charm_0..3`
(vanilla bones; the kitty FeModel merely referenced the charm chain) plus the
`purse_0_uparm_target` IK, all tuned for vanilla Mina's bag; a posed-bake
numeric diff shows the assembly lands at the lower back center within 1 unit
of its authored bind spot in idle but drifts with the purse IK in other
stances. Pinning holds the authored spot in every animation state. In-game
v10 fixed the ears and moved the keys close; the residual key offset was the
pin target (`spine_2` pitches with the chest while aiming), so **v11**
(installed, sha `c707c6f9`) splits it: bag stays on `spine_2` (underarm
clutch), keys re-pinned to `pelvis` (waist belt; hips stay upright while
aiming). Rebuilt from v9 because a reskin is lossy: v10's key verts no longer
carry gun_key lanes, so always keep the pre-reskin model. Authored
intent confirmed from GameBanana (mod 660487: design sheet + author
screenshots): keys = decorative gold chain-belt dangling at the waist over
the skirt (author's own screenshot shows them at the back-left waist), purse
= underarm book-clutch, and the mod page lists "physics problems with
decorative keys" as a KNOWN ISSUE of the source mod itself. Installed build:
Deadlock `addons/pak12_dir.vpk` = `target/baroness_mina_v10_dir.vpk` (sha
`f38b86af`); v4/v6/v7/v8/v9 preserved at
`addons/.disabled/baroness_mina_*_dir.vpk`.

This doc records the full debugging arc for the Baroness Mina combined mod
(baroness body mod + kitty ears/tail mod, recompiled into one addon via
`tools/hero-model-compiler/`), because it produced three reusable tools, two
pipeline-level findings, and a debugging lesson worth keeping.

## What the mod is

Two GameBanana mods both override `models/heroes_wip/vampirebat/vampirebat.vmdl_c`
(Mina):

- **baroness** (source preserved at `addons/.disabled/baroness_mina_source_pak12_dir.vpk`,
  sha `29c7c599`): bob haircut, pillbox hat, maid outfit, feather duster prop.
- **kitty** (`addons/.disabled/baroness_kitty_source_pak13_dir.vpk`): cat ears + tail.

Since only one addon can win a path, both models were exported to GLB, unioned
in Blender, and recompiled through `build_hero_model.py` (resourcecompiler under
Proton) with a merged skeleton (`merge_skeletons.py`). That compile path is
**mesh-only**: no embedded ANIM/ASEQ/AGRP and **no PHYS block** (the source mod
ships both). That missing PHYS turned out to be the final boss below.

## Problem 1: skeleton artifacts from the merged rig

Symptoms in the merged model (found by importing the compiled model's GLB into
Blender and auditing bones vs weights):

- The baroness **feather duster prop came along**: 1117 verts (697 handle verts
  on a custom `umbrella` bone + 420 plume verts in their own small vertex
  buffer on `feather1..4`), hanging at the model origin between her legs.
- The kitty **ear bones were displaced**: `earL/earR(+_001)` sat ~14 source
  units behind the head geometry their 199 verts were weighted to.
- None of these 10 custom bones exist in the vanilla vampirebat NM skeleton,
  so the runtime animgraph never drives them and their geometry misrenders.

**Fix** (`vpkmerge-core/examples/fix_mina_artifacts.rs`), applied to the
compiled model with the engine-proven vertex-splice primitives (no recompile):

- duster verts: collapse to a single interior point (`spine_2` bind position)
  AND re-skin onto `spine_2`. Riding a single driven bone keeps every collapsed
  triangle degenerate under any animation, so nothing rasterizes.
- ear verts: re-skin the ear-bone influence lanes onto `head` (weights
  preserved). The ears are a rigid hat part, so riding `head` is exact.

A recompile from the Blender-fixed GLB was attempted first and **abandoned**:
`vpkmerge model export` GLBs are meters-world while `build_hero_model.py`'s
rebind assumes source-unit world (the skeleton blew up 39x), and RC's output
scale barely responds to staged size, so the height error could not be iterated
away. Patching the proven compiled model was strictly safer.

## Problem 2: the bald spot (three sessions, one line of history)

A smooth pale-pink patch on the back of the head, in-game only, only really
visible while the head is pitched (aiming). The investigation graveyard, in
order, with what killed each:

| Hypothesis | Killed by |
|---|---|
| Scalp z-fighting through hair (0.5mm clearance) | 6mm scalp push (`fix_mina_scalp.rs`) changed nothing |
| Missing hair geometry / winding / UVs / weights / materials | exhaustive diff vs source model: all equivalent |
| Vanilla `mina_hair` backface culling | restoring the `F_RENDER_BACKFACES` vmat changed nothing |
| Pale region in the hair color texture | vanilla texture is uniformly dark maroon |
| Vertex-color tint defaulting white | neither model's hair carries a COLOR lane |
| Vanilla bat ears on the head mesh | the skull under the hat is a smooth bald dome |

**Plot twist found along the way:** the `F_RENDER_BACKFACES` hair vmat had
already been authored on Jul 1 as entry 26 of `baroness_mina_combined_dir.vpk`,
then silently lost when a later session used a stale no-hairfix pak12 as its
patch input. Every subsequent build inherited the loss. A 26 -> 25 **entry-count
diff across the build lineage found in minutes what model-internals diffing
missed across sessions.** When a symptom survives a fix, diff the entry lists
of the build chain first.

**Actual root cause, proven by A/B:** installing the untouched source mod
showed **no patch**, so the defect was ours. The bob's crown (8076 of 10482
hair verts) is weighted to the `hair_0/hair_1` chains, which on the source mod
are driven by FeModel cloth in the PHYS block. Our compile ships no PHYS, so
those bones are mis-driven and head animation pushes the pale skull dome
through the crown. Bind pose renders fine, which is why every offline check
passed.

**Fix** (`vpkmerge-core/examples/fix_mina_crown.rs`): re-point every
hair-chain influence lane (`hair_0/1`, `_L/_R`, `_end*`) on the `mina_hair`
draw call to `head`. The bob rides the skull rigidly, so nothing can poke
through. Trade-off: no secondary hair swing (it never worked in our build
anyway). The kitty tail keeps its `tail_*` bones.

## Build lineage

```
source pak12 (24 entries, has PHYS, no hair vmat)
  + kitty pak13                      Blender union + build_hero_model.py
    -> combined (26 entries, +mina_hair.vmat F_RENDER_BACKFACES)   Jul 1
    -> [install shuffle regressed pak12 to a no-hairfix variant]
    -> scalpfix v1/v2 (25 entries, hair vmat LOST)                 Jul 3
    -> noartifacts (duster collapsed, ears->head)     sha 74a854ff
    -> v3 = noartifacts + hair vmat restored          sha 0d33925c
    -> v4 = v3 + crown rigid to head                  sha b0c9ac51  in-game OK
combined (again; v5 forks here, dropping the no-op scalp push)
    -> fix_mina_duster (duster collapse only, EAR WEIGHTS KEPT)
    -> fix_mina_crown (bob rigid to head, as v4)
    -> patch_skeleton_binds (earL/earR(+_001) binds restored to kitty values)
    -> splice_phys (kitty PHYS block appended: ear+purse cloth + 22-body
       ragdoll/hitboxes)
    -> v5                                             sha 9bde3c93  << INSTALLED
```

## Physics restore (v5): what was actually missing, and the splice

In-game symptom after v4: "tail and ears have no physics". Findings from the
kitty source mod's own data:

- **The tail NEVER had physics, even in the source kitty mod.** Its PHYS
  FeModel drives only `earL/earR(+_001)` + the `purse_charm_*` chain (9 nodes);
  the 22 rigid bodies + 18 joints are the standard ragdoll/hitbox set; no
  `tail_*` anywhere in PHYS, ASEQ, or ANIM. The 14-bone tail chain is plain
  parented geometry riding the pelvis. Real tail swing would mean authoring new
  FeModel nodes (a rope like the purse charm), which is new cloth authoring,
  not a restore.
- **Ear cloth is restorable by splicing the donor PHYS block.** PHYS references
  bones purely **by name** (`m_boneNames`, `m_pFeModel.m_CtrlName`), and
  `m_refPhysicsData` stays empty even with an embedded PHYS, so the splice
  needs no DATA edit and no index remap. All 31 referenced bones exist in the
  merged 207-bone skeleton, and the cloth bone flags (0x43FCC8) already
  survived the recompile.
- **The block table alone is not how the engine finds embedded PHYS.** v5
  (splice only) warped the ears in-game: geometry stretched between far-apart
  transforms. The engine locates embedded physics through the **CTRL registry**
  (`embedded_physics = { phys_data_block = <block index> }`, same pattern as
  `embedded_meshes` / `embedded_animation`; confirmed on vanilla and on the
  kitty source model), and bones carrying the cloth flag (0x400000) appear to
  be left for the cloth system to write, so an unregistered PHYS leaves them
  with no transform at all and everything weighted to them warps. v6 =
  `register_embedded_phys` (byte-faithful `kv3::insert_object_member_adding`
  on CTRL) pointing at the appended block. Offline checks unaffected: ear
  binds still match kitty exactly, FeModel decodes, `embedded_meshes` intact.
- Two consequences of the missing PHYS in v4 beyond the ears: no ragdoll /
  model hitbox shapes at all, and the purse charm lost its swing too. The
  splice restores both.
- v5 also had to undo two v4 decisions scoped to the ears: the geometry weld
  (`fix_mina_duster` is `fix_mina_artifacts` minus the ear reskin, so the 199
  ear verts keep their `earL/earR(+_001)` influences) and the drifted ear
  binds (the merged skeleton had earL/earR ~14 units behind the head;
  `patch_skeleton_binds` restores kitty's exact bind pos+rot for the 4 ear
  bones so the cloth pivots sit on the geometry again). The crown stays rigid
  to head (hair cloth was a baroness FeModel we do not splice; its featherless
  duster is gone anyway).

New primitive: `morphic::resource::Resource::rebuild_with_appended_block`
(append one block, all existing blocks byte-preserved, indices stable; unit
tested). Consumed by `morphic/examples/splice_phys.rs`, which also verifies
every prior block byte-identical and the PHYS payload decodable post-splice.

## Physics restore (v7): static cloth anchors must NOT carry the cloth flag

v6 (spliced + registered PHYS) still rendered the ears and the purse charm in
the wrong place in-game. Offline audit first exonerated the data completely:
the FeModel is byte-identical to the kitty donor's, all 9 ctrl bones match the
donor binds (world-space anchor drift <= 0.02 units, the known 1.3% scale
residual), `m_InitPose` is model-space and matches the donor world binds
exactly, CTRL points at the right block, and the 22-body ragdoll equals
vanilla's set (vanilla ships PHYS but `m_pFeModel = Null`; the cloth is
entirely kitty-authored).

The defect is flag semantics, found by reading Valve's own cloth heroes:

- FeModel STATIC nodes with `m_SkelParents = -1` are **drivers**: the cloth
  system READS their transform from the animated skeleton each frame.
- The cloth flag `0x400000` on a bone EXCLUDES it from FK compose; the
  animation side leaves it for the cloth system to write (v5 proved this:
  flagged + unregistered PHYS = no transform at all).
- Our 4 static ctrls (`earL`, `earR`, `purse_charm_0`, `purse_charm_1`) are
  custom bones carrying `0x43FCC8` (flag set). Circular dependency: the cloth
  waits for animation to pose the anchor, animation skips the flagged bone.
  The anchors sit in stale bind space and both chains hang wrong.
- Valve precedent both ways: astro's scarf anchors (`scarf_0/1`) are
  **unflagged** (`0x3cc8`) hierarchy bones (only the simulated nodes are
  flagged); ghost's flagged drivers (`earring_R_0`, `feather_0`) work only
  because those bones exist in ghost's runtime rig. Our custom bones do not
  exist in vanilla vampirebat's rig, so the astro authoring is the only one
  available. The kitty donor's own embedded ANIM/AGRP carry ZERO tracks for
  the custom cloth bones, so the donor mod plausibly had this same defect
  (its ear cloth was never A/B'd standalone in-game).
- The unflagged-FK path is in-game proven in this very model: the 14 unflagged
  tail bones ride pelvis correctly.

v7 = v6 + clear `0x400000` on exactly the 4 static anchors
(`0x43FCC8 -> 0x3FCC8`; the 5 dynamic bones stay flagged for the sim), via the
new `morphic/examples/set_bone_flags.rs` (`bone=flag` pairs on
`patch_kv3_resource_scalars`). Exactly 4 bytes differ from v6, all in DATA;
PHYS/CTRL/meshes byte-identical; FeModel still == donor.

## Physics restore (v8): the donor FeModel is structurally dead; rigid FK

v7 changed nothing in-game (same floating ear / adrift key cords). A
field-population diff of the kitty FeModel against THREE working cloth models
settled it:

- References: vanilla astro (scarf), community **queen_of_hearts_mina** (the
  killer reference: SAME hero, working skirt/flag cloth, NO embedded
  anims, and custom UNFLAGGED driver bones `flag_L_1/flag_R_1`, proving both
  the astro authoring style and that custom driver bones work fine), and
  community curly_hair_wraith. All live in `addons/.disabled/`.
- kitty's FeModel is a different, incomplete dialect: `m_nStaticNodeFlags /
  m_nDynamicNodeFlags = 128/8320` vs `2176/10368` in ALL three working models
  (missing `0x800`), and ZERO entries for `m_CtrlOffsets`, `m_NodeBases`,
  `m_NodeCollisionRadii`, `m_FreeNodes`, `m_ReverseOffsets`,
  `m_TaperedCapsuleRigids`, `m_nRotLockStaticNodes` -- every one populated in
  every working model. It also uses `m_Twists` (12), which none of them do.
- Conclusion: the donor cloth never ran in the engine (plausibly broken in the
  kitty mod itself; never A/B'd standalone). The sim fails to initialize, its
  ctrl bones freeze at bind, and anything weighted to them mispositions. The
  ear verts split roughly half on `earL/earR` (FK in v7) and half on
  `earL_001/earR_001` (frozen), hence the smear.

**Keys finding** (the other user-visible symptom): the visible keys/cords are
the BARONESS keyring riding custom `gun_key_a..d` chains under
`gun_key_center <- purse_2` (4257 verts). They were never on `purse_charm_*`
(0 verts in every build): the purse_charm cloth belonged to the KITTY mod's
own satchel, an entirely different outfit whose bag the union never took.
`gun_key_*` bones are unflagged, on a driven vanilla parent chain, with binds
matching the baroness source within 0.04 units, so v8's keys render exactly
as the source mod's do. (`bone_region_histogram.rs` is the tool that found
this.)

v8 = v7 + clear `0x400000` on `earL_001/earR_001` (the real fix: the whole
ear chain now rigid-FK-rides `head`, the mechanism the tail already proves
in-game) plus the vertless `purse_charm_2/3/end` and `feather1..4/featherend`
(hygiene). Vanilla TWIST/helper bones keep the flag (vanilla-correct:
`0x400000` also marks procedural bones). The registered PHYS stays for
ragdoll/hitboxes; the dead FeModel drives nothing anyone's geometry uses.

**Offline verification** (new practice: verify placement before burning a
game restart): `model export --pose primary_stand_idle --base pak01` (a
clipless model NEEDS `--base` for donor clips or it silently bakes bind
pose), rendered via Blender MCP. Both ears sit correctly on the head, tail
natural, nothing floating. A baroness-source control bake shows exploded
duster feathers + an adrift holstered gun -- expected baker artifacts for
cloth/attachment bones, useful calibration for reading these renders.

**If someone wants real ear/key swing:** author a Valve-complete FeModel
patterned on queen_of_hearts_mina's (same hero, working, decodable with
`model femodel`). That is the deferred project; every read/patch primitive
exists, the open work is generating the full consistent array set
(NodeBases/CtrlOffsets/collision radii/SIMD lanes/node flags).

## Reusable tools this produced

- `examples/fix_mina_artifacts.rs`: kill undriven-custom-bone geometry in a
  compiled model (collapse + re-skin), template for any combined-mod import.
- `examples/fix_mina_crown.rs`: rigid-to-head reweight of cloth-chain
  influences, the cheap fix for ANY mesh-only compile whose geometry rides
  FeModel bones (hair, tassels, capes).
- `examples/vpk_add_entry.rs`: repack a dir VPK with one entry added/replaced.
- `morphic/examples/splice_phys.rs` + `register_embedded_phys.rs`: splice a
  donor PHYS (cloth + ragdoll) into a mesh-only compile (by-name bone
  validation) and register it in the CTRL registry. BOTH are required; the
  pair is the generic fix for "hero-model-compiler output has no PHYS"
  whenever a donor model exists.
- `examples/fix_mina_duster.rs`: duster-only variant of fix_mina_artifacts
  (keeps ear weights for the cloth path).
- `morphic/examples/dump_skel_info.rs`, `examples/count_bone_verts.rs`: bone
  name/flag/bind dump + weighted-vert counter, the diff tools this pass used.
- `examples/cmp_model_bbox.rs`, `examples/probe_duster_mats.rs`,
  `examples/cmp_hair_vertex_colors.rs`: small diff/probe utilities.

## Pipeline findings (apply beyond this mod)

- **hero-model-compiler output has no PHYS**: anything weighted to cloth bones
  will mis-deform in motion. Either rigid-reweight (proven here) or splice the
  donor PHYS block (unexplored, the only path to real hair/cloth swing).
- `build_hero_model.py --rebind-json` assumes the input GLB world space is
  **source units**; morphic `model export` GLBs are meters. Rescale objects to
  scale 1 (world = source units) before feeding an exported GLB back in, and
  even then expect RC output scale not to track staged size linearly.
- `morphic::model` gotchas: `Primitive.indices` index into
  `Primitive.vertex_buffer` (filter by it before histogramming); glTF JOINTS_0
  accessors span the whole buffer (count used verts via indices, not accessor
  count); `model export` adds a debug Icosphere node to every GLB (not real
  content).
- Deadlock hair secondary motion is FeModel cloth (PHYS), same as the Holliday
  tassel finding; `m_JiggleBones` stays empty on heroes.

## If someone wants hair swing back

The v5 PHYS splice answers the old open question: FeModel references bones by
**name**, no index remap needed. But hair cloth lives in the BARONESS source
model's FeModel (feather ctrls only, as shipped: the bob's swing came from the
animgraph driving the vanilla `hair_0/1` chains, which our mesh-only compile
mis-drives; that is the bald-spot root cause, see above). Hair swing back would
mean un-welding the crown and either fixing the hair-chain binds against the
runtime rig or authoring hair FeModel nodes. Only one PHYS block can win, so
merging kitty ears + any new hair cloth means merging FeModels (real authoring).

## If someone wants tail swing

New authoring, not a restore (the source mod never had it). The shape of the
work: extend the kitty FeModel with a 14-node rope for `tail..tail_013`
mirroring the purse-charm chain's node/rod/integrator pattern, then re-encode
the PHYS KV3. All the read/patch machinery exists (`model femodel` dump,
`morphic::kv3` writer); the open work is generating consistent SIMD arrays
(`m_SimdNodeBases`, `m_SimdRods`, `m_InitPose`, inv masses, stray radii) for
the added nodes.
