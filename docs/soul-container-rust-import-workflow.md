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

1. transforms GLB Y-up geometry into Source-style Z-up coordinates:
   `[x, y, z] -> [x, z, -y]`
2. computes merged mesh bounds
3. computes the stock soul-container model bounds
4. subtracts the imported mesh center
5. scales the imported mesh by:

```text
scale = stock_soul_largest_axis / imported_largest_axis
```

6. translates the mesh to the stock soul-container center

Confirmed build summaries:

```text
piplup:  6 prims -> 6 atlas groups, 6834 verts, 11082 tris, fit x0.520
togetic: 6 prims -> 3 atlas groups, 1386 verts, 1972 tris, fit x20.974
```

The Togetic installed sanity bounds were:

```text
span=[7.18318, 8.241109, 12.65]
```

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

## Current Limitations

- static mesh GLBs only
- one atlased material output
- no preservation of custom shader graphs
- no animation import for the soul container model
- complex transparency/emission/metalness are not preserved yet
- puremulti draw calls still red/error in game and remain a separate MDAT or
  model-dependency investigation
