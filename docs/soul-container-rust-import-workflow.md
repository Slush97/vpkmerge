# Soul Container Rust Import Workflow

Date: 2026-06-14

This is the runtime-confirmed Rust path for importing a static `.glb` as the
Deadlock soul container addon. It does not use ModelDoc or `resourcecompiler.exe`
for the installed artifact.

## What "Rust Import" Means

The working path is Rust-generated and Rust-repacked:

- read the user `.glb` in Rust
- merge all GLB primitives into one replacement mesh
- atlas material groups into one color texture
- center and auto-scale the mesh to the stock soul-container bounds
- graft the mesh into the stock soul-container `.vmdl_c` envelope
- preserve the stock packed vertex layout the engine expects
- patch the model draw call to a unique material path
- patch a committed donor `.vmat_c` to point at the generated atlas texture
- optionally ship recolored soul particle overrides
- pack the addon `.vpk` in Rust

It is still a pragmatic hybrid asset pipeline: the model envelope is the stock
soul-container model, and the material starts from a committed precompiled donor
VMAT template. The success is that the import/build/repack/install path itself is
Rust-only and does not require Valve tooling for each imported GLB.

## Confirmed Inputs

These both rendered in game after installing as `pak43_dir.vpk`:

```text
/home/esoc/Downloads/piplup.glb
/home/esoc/Downloads/togetic.glb
```

Confirmed artifacts:

```text
/tmp/piplup_clone_packed_dir.vpk
sha256 39c48e158f8e877d8ac9a4c40c044576ef989928b23959624135a69a259046f3

/tmp/togetic_clone_packed_dir.vpk
sha256 98dd9c305243377ad84b12c654c2e4a9cf833fc9fe5332cac3f05b40de1a7d88
```

## Build Command

General form:

```bash
cargo run --release --example soul_import_clone -- \
  /home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk \
  /path/to/model.glb \
  /tmp/<name>_clone_packed_dir.vpk \
  <name>
```

Piplup:

```bash
cargo run --release --example soul_import_clone -- \
  /home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk \
  /home/esoc/Downloads/piplup.glb \
  /tmp/piplup_clone_packed_dir.vpk \
  piplup
```

Togetic:

```bash
cargo run --release --example soul_import_clone -- \
  /home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk \
  /home/esoc/Downloads/togetic.glb \
  /tmp/togetic_clone_packed_dir.vpk \
  togetic
```

## Install Command

Install into the current addon proof slot:

```bash
cp /tmp/<name>_clone_packed_dir.vpk \
  /home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/addons/pak43_dir.vpk
```

Verify the installed file matches the built artifact:

```bash
sha256sum /tmp/<name>_clone_packed_dir.vpk \
  /home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/addons/pak43_dir.vpk
```

## Validation Gates

Before installing, run:

```bash
cargo run --quiet -p vpkmerge-core --example v0sanity -- \
  /tmp/<name>_clone_packed_dir.vpk \
  models/props_gameplay/soul_container/soul_container.vmdl_c
```

The important checks are:

- finite bounds
- largest bounds axis around `12.65`
- one embedded mesh
- one draw call
- no red/error material in game
- no line artifacts from vertex-layout drift

For code changes to the importer/layout path, also run:

```bash
cargo check -p morphic -p vpkmerge-core --example soul_import_clone
cargo test -p morphic model::mesh::assemble_tests::assemble_to_layout_preserves_soul_packed_frame_layout --lib
```

The staged commit that introduced the working path was also checked from an
isolated `git archive` snapshot before commit.

## Auto-Scale Behavior

The script auto-scales. After reading and merging GLB primitives, it:

1. applies each GLB node's world transform to positions and normals
2. transforms GLB Y-up geometry into Source-style Z-up coordinates:
   `[x, y, z] -> [x, z, -y]`
3. applies the selected import orientation
4. computes merged mesh bounds
5. computes the stock soul-container model bounds
6. subtracts the imported mesh center
7. scales the imported mesh by:

```text
scale = stock_soul_largest_axis / imported_largest_axis
```

8. translates the mesh to the stock soul-container center

Confirmed build summaries:

```text
piplup:  6 prims -> 6 atlas groups, 6834 verts, 11082 tris, fit x0.520
togetic: 6 prims -> 3 atlas groups, 1386 verts, 1972 tris, fit x20.974
```

The Togetic installed sanity bounds were:

```text
span=[7.18318, 8.241109, 12.65]
```

## Orientation Overrides

The importer accepts explicit orientation controls for GLBs whose baked up-axis or
root orientation does not match the default path:

```bash
SOUL_ORIENT=y-up    # default, after GLB node-world transforms
SOUL_ORIENT=z-up    # rotate a Z-up source so game Z is tallest
SOUL_ORIENT=flip-y  # flip the post-conversion vertical sign
SOUL_ORIENT=auto    # pick y-up or z-up by largest game-Z bounds span
SOUL_ROTATE=90,0,0  # optional extra X,Y,Z Euler rotation in degrees
```

`auto` is intentionally bounds-only. It handles common y-up versus z-up cases, but
explicit `SOUL_ORIENT` / `SOUL_ROTATE` is still the reliable path for upside-down,
symmetrical, or deliberately wide models.

## Vertex Layout Fix

The first single-draw clone rendered but had severe line artifacts. The cause was
layout drift: the assembler rewrote the stock soul-container buffer from the
engine-expected packed layout into wider float UV/normal fields while the draw
call still requested compressed tangent frames.

The working layout preserves:

```text
stride 24
POSITION     format 6   offset 0
TEXCOORD     format 37  offset 12  LowPrecisionUv
NORMAL       format 42  offset 16  CompressedTangentFrame
BLENDINDICES format 30  offset 20
```

Regression test:

```text
model::mesh::assemble_tests::assemble_to_layout_preserves_soul_packed_frame_layout
```

## Particle Modes

The importer currently handles the three soul glow particle files:

```text
particles/generic/holding_gold_neutral_model.vpcf_c
particles/generic/holding_gold_neutral_model_glow.vpcf_c
particles/generic/holding_gold_neutral_embers.vpcf_c
```

Modes:

```bash
SOUL_GLOW=recolor  # default: hue-shift shipped particle overrides to the GLB dominant color
SOUL_GLOW=base     # ship unchanged base particle overrides
SOUL_GLOW=off      # do not ship particle overrides
```

`SOUL_GLOW=off` is not a true mute; the base game particle paths still resolve.
A true mute should be implemented as a new mode, likely `SOUL_GLOW=mute`, that
ships inert overrides for those three particle paths or patches spawn/intensity
or alpha to zero.

## Albedo Atlas Resolution (and a dead end)

The albedo atlas is spliced into a same-size BCn donor texture
(`dev/helper/testgrid_color_tga_2d6cc34.vtex_c`, 512x512) via `replace_mip_chain`,
which can only overwrite pixels in a donor of matching dimensions. That hard-caps
the whole atlas at 512x512 split across every material group (six groups -> ~170px
cells). This is the main albedo-resolution limit.

**Dead end, do not retry without a new plan:** minting the atlas from scratch with
`morphic::encode_vtex_png_rgba8888` (inline `PNG_RGBA8888`, single mip) lifts the
512 cap and decodes cleanly offline (it round-trips out of the packed VPK via
`vpkmerge texture --from-vpk`, and Grimoire's own preview renders it correctly).
But **Deadlock's engine REJECTS an inline-PNG_RGBA8888 albedo on a model material
and renders the missing-texture purple** (in-game verified on piplup: flat purple
body, correct only in the offline / Grimoire decoders). `PNG_RGBA8888` is a
UI/panorama format; shipped model albedos are all BCn with mip chains. So the
resolution bump cannot go through the inline-PNG writer.

To actually raise albedo resolution the atlas must stay BCn, which means a **larger
same-format BCn donor** (e.g. a committed 2048 BC7 `.vtex_c` to splice into). That
is the open follow-up; the code stays on the proven 512 BCn donor for now.

## Orientation Auto-Picker

`SoulOrient::Auto` scores candidate rotations about X (identity, +/-90, 180) in the
FINAL mesh space (after the assembler's `[x, z, -y]` swizzle), preferring the
tallest vertical axis and then the most bottom-heavy (right-side-up) result. The old
picker only tried identity and +90, measured the pre-swizzle span, and never checked
the up-sign, so it both missed the rotation Sketchfab models need and could land a
model upside-down.

Sketchfab GLBs are the common case: the `Sketchfab_model` root node carries a
Z-up -> Y-up matrix, and combined with the importer's conversion the model needs an
extra **-90 about X** to stand up (`SOUL_ROTATE=-90,0,0`, or just `SOUL_ORIENT=auto`,
which resolves to `auto:z-up-inv`). Explicit `SOUL_ROTATE` remains the reliable
override for ambiguous models.

## Current Limitations

- static mesh GLBs only
- one atlased material output
- no preservation of custom shader graphs
- no animation import for the soul container model
- complex transparency/emission/metalness are not preserved yet
- puremulti draw calls still red/error in game and remain a separate MDAT or
  model-dependency investigation
