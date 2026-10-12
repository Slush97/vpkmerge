# Fixtures

Two tiers:

- **Canonical (committed):** small textures grouped by format under
  `<format>/`. Each ships with a sibling `.png` (oracle output) and
  `.meta.json` (format, dims, source SHA, tolerance). The Rust golden harness
  diffs morphic's decode against these and verifies the source SHA, so CI is
  immune to silent fixture drift.

- **Local (gitignored):** `_local/` holds the extended corpus extracted from
  the user's Deadlock install via `just bootstrap`. Used for stress testing
  before declaring a milestone done. Not in CI.

## Layout

```
fixtures/
  rgba8/                  RGBA8888 + BGRA8888 (M2)
  dxt1/                   DXT1 = BC1 (M3)
  dxt5/                   DXT5 = BC3 (M4)
  ati1n/, ati2n/          BC4, BC5 (M5)
  bc7/                    (M6)
  bc6h/                   (M7)
  rgba16f/                (M8)
  kv3/                    raw KV3 blocks + .kv3.json goldens (model exporter M1)
  meshopt/                raw MVTX/MIDX blocks + .meshopt.json goldens (M2)
  nm/                     whole .vnmclip_c resources for the NM pose codec
  particle/               whole .vpcf_c resources for the repoint tests
  _local/                 gitignored, oracle-populated stress corpus
```

The `kv3/` tier differs from the texture tiers: each `*.kv3bin` is a raw KV3
block sliced out of a `.vmdl_c`, and its sibling `*.kv3.json` is the canonical
JSON the morphic KV3 parser is diffed against (see `tools/morphic-oracle`,
`kv3-dump`). Re-bless with `just kv3-goldens`. The committed blocks are
hornet's `DATA` (model skeleton + remap tables + LOD masks), `MDAT[0]` (the
body mesh's draw calls + scene bounds), and `CTRL` (the embedded-mesh buffer
registry: vertex/index layouts + meshopt flags).

The `material/` tier holds a full committed `.vmat_c` resource (not a sliced
block) plus a `*.material.json` golden from `morphic-oracle material-meta` (VRF's
shader name + int/float/vector/texture parameter tables). `morphic::material::parse`
is diffed against it. Re-bless with `just material-meta`.

`material/necro_hands.vmat_c` has no `*.material.json` golden; it is a **blobbed**
material (KV3 v5, LZ4, `countBlocks = 1`) used by `kv3::patch` tests to exercise
the in-place tint-double recolor that keeps a blob section compressed (see
`docs/spike-blobbed-vmat-recolor.md`). `material/necro_picker_hand_effect.vmat_c`
is a **two-blob** material (`countBlocks = 2`, its `m_dynamicParams` expressions)
that regresses the blob-frame reader's one-frame-per-blob path.
`material/picker_hand_glow.vmat_c` has a **neutral white** `g_vColorTint` stored as
tagless `DOUBLE_ONE`s, exercising the recolor's re-encode promotion path.
`material/necro_gravestone.vmat_c` is a **solid (non-additive)** prop with a white
base `g_vColorTint` and a colored `g_vSelfIllumTint` (the skull/RIP/cracks glow),
exercising the selective stamp that leaves the stone base neutral and recolors only
the glow. None of these has a `*.material.json` golden.
`material/haze_hair_cable.vmat_c` is a small plain **KV3 v4** material (over half of
shipped particles are v4 too).

The `particle/` tier holds whole `.vpcf_c` resources for `repoint_resource_refs`
(`tests/repoint.rs`): `grenade_frag_fx` (v4; a texture and a child particle),
`wraith_rapid_fire_cast_proj` (v4; a material), `wraith_rapid_fire_buff` (v5;
three child particles) and `wraith_ult_range_indicator_projected` (v5; a
material). Extracted unmodified from `citadel/pak01`.

`kv3/hornet_model_meta.json` is a higher-level golden (not a raw block): the
compact model summary the M3 mesh/skeleton decoder is diffed against, produced
by `morphic-oracle model-meta`. It holds the sorted bone-name set, per-LOD0-mesh
vertex layouts + draw calls + materials + scene bounds, the vertex/index totals,
and a source-space position bbox. The committed `model::tests` reproduce the
buffer-free parts of it from the `DATA`/`CTRL`/`MDAT[0]` blocks above; the gated
`tests/model_local.rs` reproduces the whole thing from a real VPK. Re-bless with
`just model-meta`.

The `meshopt/` tier holds raw `*.meshopt` MVTX/MIDX payloads sliced from
hornet's embedded meshes, each with a sibling `*.meshopt.json` golden
(decoded length + SHA-256 from VRF). The morphic meshopt decoders are diffed
byte-for-byte against these. Committed fixtures span vertex strides 52/56/60
and both index buffers. Re-bless with `just mesh-buffers`.

The `nm/` tier holds whole `.vnmclip_c` resources (not sliced blocks) for the
NM quantized-pose codec (`m_compressedPoseData`). The golden is intrinsic, no
oracle JSON: `tests/nm_clip.rs` decodes each clip, re-encodes the pose stream,
and asserts the re-encode is byte-identical and a re-decode reproduces the
tracks. The four committed clips are all yamato's: `rope_climb_idle`
(translation-only animated), `reload_idle_quick` (rotation+translation; the
press-R taunt slot), `flinch_back` (additive), and `ui_hero_select` (fully
static, the no-compressed-stream degenerate case). The animated-scale channel
(rare; no yamato clip uses it) is covered by a `model::nm` unit test, and the
broad pak-wide round-trip by the gated `tests/nm_clip_local.rs`.

## Provenance

| Path                                          | Source (Deadlock pak01 entry)                                        |
|-----------------------------------------------|----------------------------------------------------------------------|
| `rgba8/minimap_circle.vtex_c`                 | `materials/minimap/minimap_circle.vtex_c`                            |
| `dxt1/default_color_tga_99901565.vtex_c`      | `materials/default/default_color_tga_99901565.vtex_c`                |
| `kv3/hornet_data.kv3bin`                      | `DATA` block of `models/heroes_staging/hornet_v3/hornet.vmdl_c`      |
| `kv3/hornet_mdat0.kv3bin`                     | `MDAT[0]` block of `models/heroes_staging/hornet_v3/hornet.vmdl_c`   |
| `kv3/hornet_ctrl.kv3bin`                      | `CTRL` block of `models/heroes_staging/hornet_v3/hornet.vmdl_c`      |
| `kv3/hornet_model_meta.json`                  | `model-meta` summary of `models/heroes_staging/hornet_v3/hornet.vmdl_c` |
| `material/vindicta_headv2.vmat_c`             | `models/heroes_staging/hornet_v3/materials/vindicta_headv2.vmat_c`  |
| `material/necro_hands.vmat_c`                 | `models/abilities/materials/necro_hands.vmat_c`                     |
| `material/necro_picker_hand_effect.vmat_c`    | `models/heroes_wip/necro/materials/necro_picker_hand_effect.vmat_c` |
| `material/picker_hand_glow.vmat_c`            | `models/heroes_wip/necro/materials/picker_hand_glow.vmat_c`         |
| `material/necro_gravestone.vmat_c`            | `models/heroes_wip/necro/materials/necro_gravestone.vmat_c`         |
| `nm/yamato_rope_climb_idle.vnmclip_c`         | `models/heroes_wip/yamato/clips/rope_climb_idle.vnmclip_c`          |
| `nm/yamato_reload_idle_quick.vnmclip_c`       | `models/heroes_wip/yamato/clips/reload_idle_quick.vnmclip_c`        |
| `nm/yamato_flinch_back.vnmclip_c`             | `models/heroes_wip/yamato/clips/flinch_back.vnmclip_c`              |
| `nm/yamato_ui_hero_select.vnmclip_c`          | `models/heroes_wip/yamato/clips/ui_hero_select.vnmclip_c`           |

Extracted via `tools/morphic-oracle` from a local Steam install. Re-extract
with `just fixture <entry> <subdir>`. See `tools/format-counts.csv` for the
full pak01 inventory.

## Copyright

The committed canonical corpus is intentionally tiny so it serves as a
regression test, not redistribution. Where possible, prefer hand-authored
synthetic textures (gradient, checker, alpha edge) over game assets. The
extended `_local/` corpus is per-user and never leaves the developer's
machine.
