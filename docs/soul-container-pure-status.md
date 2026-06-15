# Soul Container Pure Compiler Status

Date: 2026-06-14

## Last Confirmed VMAT/VTEX Result

`pak43_dir.vpk` rendered successfully in game.

Trial install path at the time:

```text
/home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/addons/pak43_dir.vpk
```

The current addons directory may no longer contain that slot; the trial result is
kept here as evidence for the material/texture path.

This VPK is still a hybrid probe, not a complete pure replacement:

- Model geometry: CSDK/oracle `soul_container.vmdl_c`
- Default textures: CSDK/oracle default texture resources
- Color textures: pure Rust `PNG_RGBA8888` `.vtex_c`
- VMATs: oracle `RERL`, `RED2`, and `INSG`; oracle v5 DATA layout patched
  byte-faithfully so `g_tColor` points at deterministic pure texture paths

Local validation passed before install:

- `v0sanity`
- 18 entries
- 6919 vertices
- 6 material refs
- meshopt true
- largest bounds axis around `12.65`

Latest model-side follow-up:

- `/tmp/piplup_puremulti_dir.vpk` rebuilt with corrected draw-call ranges still
  hit the in-game red/error render.
- `/tmp/piplup_puremulti_oraclemats_dir.vpk` and
  `/tmp/piplup_puremulti_oraclemats_modeldeps_dir.vpk` also red/error, so
  swapping in the known-rendering VMAT/VTEX set and oracle model `RERL`/`RED2`
  did not make the multi-draw edit engine-loadable.
- A model `DATA` copy experiment failed local `v0sanity` with
  `blend index out of remap range`; it was not installed.
- The likely first puremulti bug was model `MDAT`: cloned draw calls kept
  `m_nAppliedIndexOffset = 0` and `m_nVertexCount = total_vertex_count` for
  every material, unlike resourcecompiler's per-material vertex ranges.
- `morphic` now decodes draw calls with `m_nAppliedIndexOffset`, and
  `set_draw_call_groups` writes `m_nAppliedIndexOffset` plus exclusive
  `m_nVertexCount` bounds.
- `soul_import_puremulti` now writes group-local indices and cumulative vertex
  ranges. The regenerated Piplup ranges are:

```text
initialshadinggroup  indices 0..2037      vertices 0..551
lambert4sg           indices 2037..4266   vertices 551..1079
lambert5sg           indices 4266..9270   vertices 1079..2133
lambert6sg           indices 9270..11190  vertices 2133..2553
lambert8sg           indices 11190..18774 vertices 2553..4093
lambert7sg           indices 18774..33246 vertices 4093..6834
```

- `/tmp/piplup_clone_dir.vpk` (single draw call, atlased material) rendered in
  game, proving the addon slot, VMAT, VTEX, particles, and single-draw graft path
  can load. It showed severe line/geometry artifacts.
- The line-artifact diagnosis was vertex-layout drift: `assemble_to_layout`
  changed the stock soul buffer from stride 24
  (`POSITION`/`R16G16_SNORM TEXCOORD`/`R32_UINT NORMAL` packed frame/
  `R8G8B8A8_UINT BLENDINDICES`) to stride 36 with float UV and float normal,
  while the draw call and material still requested compressed tangent frames.
- `assemble_to_layout` now preserves writable target encodings, including
  `R16G16_SNORM` UVs and `R32_UINT` packed normal/tangent frames. New regression:
  `assemble_to_layout_preserves_soul_packed_frame_layout`.
- Confirmed PASS artifact:

```text
/tmp/piplup_clone_packed_dir.vpk
sha256 39c48e158f8e877d8ac9a4c40c044576ef989928b23959624135a69a259046f3
```

Local validation passes (`v0sanity`, full `morphic` lib tests, example check).
In-game result: renders correctly with no red/error shader and without the
previous severe line artifacts.

This is the first confirmed runtime-successful model import built by the Rust
path, not resourcecompiler/ModelDoc. It is still a pragmatic hybrid: Rust grafts
the GLB geometry into the stock soul-container model envelope, builds the atlas
texture/material references, repacks particles, and installs the VPK; it uses a
committed precompiled donor VMAT template rather than synthesizing a fully new
engine-accepted VMAT from empty bytes.

Current installed artifact:

```text
/tmp/togetic_clone_packed_dir.vpk
installed as /home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/addons/pak43_dir.vpk
sha256 98dd9c305243377ad84b12c654c2e4a9cf833fc9fe5332cac3f05b40de1a7d88
```

Togetic build summary: `/home/esoc/Downloads/togetic.glb`, 6 primitives -> 3
atlas groups, 1386 vertices, 1972 triangles, auto-fit scale `x20.974`, 512px
atlas, particles recolored to hue 44 degrees. Local installed `v0sanity` passes;
in-game result: PASS.

## Runtime Probe History

`pak39_dir.vpk`

- Oracle model plus pure DATA-only VMAT and pure VTEX.
- Failed in game: red/wireframe material.

`pak40_dir.vpk`

- Oracle model plus oracle VMAT and pure VTEX at CSDK hash-suffixed paths.
- Rendered in game as expected.
- Proved pure `PNG_RGBA8888` VTEX is engine-accepted.

`pak41_dir.vpk`

- Oracle model plus pure VMAT with generated `DATA+INSG` and pure VTEX.
- Failed in game: red/wireframe material.
- Proved `INSG` alone is not enough.

`pak42_dir.vpk`

- Oracle model plus oracle `RERL/RED2/INSG`, but VMAT DATA fully regenerated
  through morphic's normal KV3 encoder.
- Failed in game: red/wireframe material.
- Proved full v4 DATA re-encode is not engine-accepted, even with oracle
  non-DATA blocks.

`pak43_dir.vpk`

- Oracle model plus oracle `RERL/RED2/INSG`; VMAT DATA remains v5 and is patched
  byte-faithfully only at `g_tColor`.
- Rendered in game.
- Proves engine-compatible VMAT output needs v5 DATA preservation/construction,
  not a lossy full v4 KV3 re-encode.

`/tmp/piplup_puremulti_dir.vpk`

- Stock soul-container model envelope plus pure uncompressed grafted geometry,
  six donor-patched v5 VMATs, and six pure `PNG_RGBA8888` textures.
- The first version was suspected to error in game.
- Rebuilt after fixing draw-call vertex-range semantics as described above.
- In-game result: red/error render.

`/tmp/piplup_clone_dir.vpk`

- Single draw call with atlased material.
- In-game result: rendered, but with severe line/geometry artifacts.
- Proved single-draw model replacement and material binding can load.
- Superseded by `/tmp/piplup_clone_packed_dir.vpk`.

`/tmp/piplup_clone_packed_dir.vpk`

- Same single-draw atlas strategy as `/tmp/piplup_clone_dir.vpk`.
- Preserves the stock soul-container vertex buffer contract:
  stride 24, `TEXCOORD` format 37 at offset 12, `NORMAL` format 42 at offset
  16 with `CompressedTangentFrame`, and `BLENDINDICES` format 30 at offset 20.
- Installed into `pak43_dir.vpk`.
- In-game result: PASS. Renders cleanly.

`/tmp/togetic_clone_packed_dir.vpk`

- Built from `/home/esoc/Downloads/togetic.glb`.
- Same single-draw atlas strategy and packed stock vertex layout.
- Build summary: 6 primitives -> 3 atlas groups, 1386 vertices, 1972 triangles,
  auto-fit scale `x20.974`, normalized bounds span `[7.18318, 8.241109, 12.65]`.
- Installed into `pak43_dir.vpk`.
- In-game result: PASS. Renders cleanly.

## Implementation State

Already implemented:

- `morphic::encode_vtex_png_rgba8888`
- `morphic::encode_vtex_png_rgba8888_from_png`
- `morphic::encode_pbr_vmat_c`
- generated PBR `INSG` block in `encode_pbr_vmat_c`
- partial `SoulContainerCompileBackend::PureRust` material/texture packing
- `morphic::model::set_draw_call_groups` with resourcecompiler-style
  per-draw-call vertex ranges
- probe examples:
  - `soul_container_pure_probe`
  - `kv3_block_dump`
  - `vmat_template_redata`
  - `vmat_template_redirect_color`
  - `soul_import_puremulti`

Important caveat:

`encode_pbr_vmat_c` still emits v4 DATA. It parses locally, but the game rejects
that material DATA. It should not be considered engine-accepted yet.

## Next Step

Implement VMAT emission using the byte-faithful v5 path:

1. Start from a compact donor/template VMAT for `pbr.vfx`.
2. Patch `m_materialName`, `m_textureParams[*]`, representative dimensions, and
   required flags with the existing KV3 byte-faithful patchers.
3. Generate or patch `RERL` to include the same texture resource refs.
4. Generate or patch `RED2` so child/dependency lists agree with the VMAT DATA.
5. Repack a new slot and test in game before moving on to pure `.vmdl_c`.

Pure `.vmdl_c` is still not implemented. Keep using oracle/resourcecompiler
model geometry for runtime material/texture probes.

For the model-side path, the immediate production candidate is now the
single-draw atlased importer. Puremulti remains a separate
MDAT/resource-dependency investigation.
