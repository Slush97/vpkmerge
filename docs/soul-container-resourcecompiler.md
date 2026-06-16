# Soul Container Resourcecompiler Findings

This is the path that produced a real Source 2-compiled soul container from a
custom `.glb`, including compiled model, compiled materials, compiled textures,
and meshopt-compressed buffers.

## Bottleneck

The Rust GLB importer path in `soul_import_clone.rs` grafts geometry/material
data into an existing compiled model. That can make a custom-looking container,
but it is not the same as building a brand-new Source 2 model resource.

For a full replacement model, the pipeline needs to be:

1. Convert GLB to Source-friendly FBX.
2. Generate source `.vmat` files and source textures.
3. Generate a source `.vmdl` under the target model path.
4. Run `resourcecompiler.exe`.
5. Pack the compiled `game/citadel_addons/<addon>` output into a dir VPK.

The maintained dev-time wrapper for this is:

```sh
tools/soul-container-compiler/build_soul_container.py <input.glb> \
  --addon <addon_name> \
  --output <out_dir.vpk> \
  --force
```

See `tools/soul-container-compiler/README.md` for install and environment
options.

## Rust Prep API

The first Blender dependency has been factored into `vpkmerge-core`:

```rust
vpkmerge_core::prepare_soul_container_import(glb, source_root, options)
```

It reads the GLB with the Rust `gltf` crate, applies node transforms and the
same center-and-fit rule, writes source `.vmat` files, source PNG color
textures, `soul_container.vmdl`, and a static `model.fbx` with FBX material
names set to Source-relative material paths:

```text
models/props_gameplay/soul_container/materials/<material>
```

The prepared output is intentionally a compiler source tree, not hand-written
compiled resources. The matching abstraction is:

```rust
vpkmerge_core::compile_soul_container_source(source_root, options, backend, output_vpk)
```

The full end-to-end backend is still
`SoulContainerCompileBackend::ResourceCompiler`. `PureRust` is partial: it emits
generated `.vmat_c` / `.vtex_c` resources, but not `.vmdl_c`.

Manual prep probe:

```sh
cargo run -p vpkmerge-core --example prepare_soul_container -- \
  /home/esoc/Downloads/piplup.glb /tmp/piplup_soul_source
```

Important validation boundary: the Rust prep API has unit coverage for GLB
parsing, material path generation, source file emission, and normalized bounds.
The minimal Rust-authored FBX still needs resourcecompiler and in-game probes
against several real GLBs before it replaces the Blender-generated FBX path in
the installed proof pipeline.

Current probe: resourcecompiler accepts the Rust-prepared `.vmdl` but emits only
a tiny modeldoc shell (`CTRL has no embedded_meshes`) for both an ASCII FBX and
a minimal Kaydara binary FBX 7400. Blender's FBX remains the known-good
compiler input. The next Rust-prep milestone is therefore FBX SDK compatibility:
diff Blender's binary FBX node/property graph against the Rust writer and keep
adding required FBX structures until resourcecompiler emits VBIB/material/VTEX
outputs.

## Rust FBX Compatibility Harness

Three Track-1 examples now gate the Rust prep path without touching the installed
proof VPK:

```sh
cargo run -p vpkmerge-core --example fbx_graph_diff -- \
  /home/esoc/csdk12/Reduced_CSDK_12/content/citadel_addons/test/models/props_gameplay/soul_container_glbprobe/model.fbx \
  /tmp/<rust-prep>/models/props_gameplay/soul_container/model.fbx \
  --max-diffs 120 --dump-focus
```

This parses both binary FBX files with `fbxcel`, loads DOM object metadata with
`fbxcel-dom`, and compares root paths, node/property signatures, object
class/subclass counts, connection kinds, and focused sections:
`GlobalSettings`, `Definitions`, `Objects/Geometry`, `Objects/Model`,
`Objects/Material`, `Connections`, and `Takes`.

```sh
cargo run -p vpkmerge-core --example soul_container_acceptance -- \
  /home/esoc/Downloads/piplup.glb /tmp/piplup_rust_accept_dir.vpk \
  --addon piplup_rust_accept
```

This runs:

1. Rust `prepare_soul_container_import`.
2. `resourcecompiler.exe` through Proton.
3. VPK packing.
4. VPK/resource inspection.

It fails unless the compiled model decodes with embedded meshes, material refs
are Source-relative under the expected model folder, draw-call/material count
matches the prepared material count, largest bounds axis is `12.65 +/- 0.05`,
and `.vmat_c` / `.vtex_c` outputs exist. It explicitly refuses to write:

```text
/home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/addons/pak06_dir.vpk
```

The controlled mutation harness starts from the known-good Blender source model
directory, swaps the staged `model.fbx` one feature at a time toward a candidate
Rust FBX, runs resourcecompiler/pack/inspect for each case, and prints
`embedded` versus `shell` outcomes:

```sh
cargo run -p vpkmerge-core --example soul_container_fbx_mutation -- \
  /tmp/<rust-prep>/models/props_gameplay/soul_container/model.fbx \
  --out-dir /tmp/vpkmerge-fbx-mutation
```

Use `--write-only` to generate the mutated FBX files without invoking
resourcecompiler. The default cumulative run includes baseline raw FBX,
baseline fbxcel roundtrip, metadata/material swaps, compatible geometry/model
swaps, full `Objects` + `Connections`, and the fully rewritten candidate. If
the candidate has different `Objects/Geometry` or `Objects/Model` counts, the
default run skips those count-sensitive feature swaps and says why; pass
`--features geometry_edges,geometry_normals,...` explicitly when comparing a
candidate with matching object counts.

The harness rewrites candidate material/model strings from
`models/props_gameplay/soul_container` to the known-good proof path
`models/props_gameplay/soul_container_glbprobe` before compiling, so each
mutation uses the proof VMAT/PNG/VMDL source tree and changes only the FBX under
test. It writes output VPKs under `--out-dir` and carries the same installed
proof VPK refusal guard as the acceptance example.

Current Rust binary FBX improvements:

- Valid FBX 7.4 footer (`fbxcel` reports `footer=ok`).
- Blender-style object metadata strings: `name\0\x01Class`.
- One `Geometry/Mesh` and one `Model/Mesh` per material instead of one
  multi-material geometry.
- `LayerElementMaterial` uses `AllSame`.
- Normal and UV layers use `IndexToDirect` with index arrays.
- Blender-style null model hierarchy and `NodeAttribute/Null` objects.
- Blender-observed global axis/time/default-camera fields.
- Root null rotation/scale matching the Blender export.
- Basic `Definitions` property templates.
- `Geometry/Properties70` and generated `Edges` arrays.
- Blender-style top-level `FileId`, `CreationTime`, and `Creator` root nodes.

The controlled mutation harness found the resourcecompiler flip point: with the
current Rust FBX graph, cumulative mutations stayed embedded through header,
global settings, documents, definitions, material properties, model properties,
geometry edges, normals, UVs, material layers, topology, full geometry payloads,
and full `Objects` + `Connections`. The fully rewritten candidate only shelled
when it lacked Blender's three top-level metadata nodes: `FileId`,
`CreationTime`, and `Creator`.

After adding those nodes to the Rust writer, the acceptance harness passes:

```text
compiled addon piplup_rust_accept_metadata packed_entries 18 output /tmp/piplup_rust_accept_metadata_dir.vpk
validated embedded geometry: mesh_parts=1 draw_calls=6 materials=6 bounds_span=[12.650001, 9.299569, 8.924238] vmat_c=6 vtex_c=11
acceptance ok
```

The release `v0sanity` gate also passes against that VPK:

```text
span=[12.650001, 9.299569, 8.924238]
```

## Compiler Oracle Fixtures

Before replacing `resourcecompiler.exe`, freeze compiler outputs for the small
GLB corpus. The fixture freezer keeps the Rust-prepared source tree, copied
CSDK compiled game tree, packed VPK, `v0sanity.txt`, and structured
`oracle.json` metadata with model bounds, draw/material refs, resource blocks,
compiled material params, and VTEX formats:

```sh
cargo run -p vpkmerge-core --example soul_container_oracle_fixtures -- \
  --out-dir /tmp/soul-container-oracle \
  --case piplup=/home/esoc/Downloads/piplup.glb \
  --case cinna=/home/esoc/Downloads/75ee040c5394475481652b9064889728.glb \
  --case edge_chest=/home/esoc/aaplsucks/assets/dungeon/chest.glb \
  --case edge_cesium_man=/home/esoc/aaplsucks/assets/models/CesiumMan.glb \
  --case edge_fox=/home/esoc/aaplsucks/assets/models/Fox.glb \
  --force
```

Use additional `--case name=/path/model.glb` arguments for edge cases. The tool
guards the known installed addon proof paths and writes fixture output under
`--out-dir`, not into the Deadlock install.

## Pure Rust VTEX Writer

The first pure-Rust compiled-resource writer is intentionally narrow:
`morphic::encode_vtex_png_rgba8888` and
`morphic::encode_vtex_png_rgba8888_from_png` build a complete `.vtex_c` from
scratch using inline `PNG_RGBA8888` payloads. This does not try to match CSDK's
BC7 texture choices yet; it gives the pure backend a small engine-plausible
texture resource to validate before VMAT/VMDL emission lands.

Developer probe against a prepared source PNG:

```sh
cargo run -p morphic --example write_vtex_png -- \
  /tmp/soul-container-oracle/piplup/source/models/props_gameplay/soul_container/materials/initialshadinggroup_color.png \
  /tmp/piplup_inline_png.vtex_c
```

## Pure Rust VMAT Writer

The first pure-Rust `.vmat_c` writer is similarly constrained:
`morphic::encode_pbr_vmat_c` emits a generated material for the soul-container
`pbr.vfx` subset. It writes a `DATA` KV3 block with the CSDK-observed material
fields for `m_materialName`, `m_shaderName`, `m_intParams`,
`m_vectorParams`, `m_textureParams`, empty dynamic/attribute arrays, and
representative texture dimensions. It also emits the static `INSG` shader input
signature observed across the CSDK oracle corpus. The color slot is
caller-provided as `g_tColor`; missing PBR/NPR slots use the same default
material texture paths CSDK inserted in the oracle fixtures.

Developer probe:

```sh
cargo run -p morphic --example write_vmat -- \
  models/props_gameplay/soul_container/materials/initialshadinggroup.vmat \
  models/props_gameplay/soul_container/materials/initialshadinggroup_color.vtex \
  2 2 \
  /tmp/piplup_generated.vmat_c
```

## Partial Pure Rust Backend Probe

`SoulContainerCompileBackend::PureRust` now performs the partial material and
texture compile: it scans a prepared source tree and packs generated `.vmat_c`
and `.vtex_c` resources. It intentionally does not emit `soul_container.vmdl_c`
yet, so the output is not a complete replacement VPK.

For a fresh GLB prepare + pure material/texture compile:

```sh
cargo run -p vpkmerge-core --example soul_container_pure_probe -- \
  /tmp/soul-container-oracle/piplup/input.glb \
  /tmp/soul-container-pure-probe/piplup/compiled_game \
  /tmp/soul-container-pure-probe/piplup/piplup_pure_dir.vpk \
  --oracle /tmp/soul-container-oracle/piplup/oracle.json \
  --force
```

For a frozen prepared source tree, useful when the fixture GLB references an
external texture URI:

```sh
cargo run -p vpkmerge-core --example soul_container_pure_probe -- \
  --source-root /tmp/soul-container-oracle/edge_chest/source \
  /tmp/soul-container-pure-probe/edge_chest/compiled_game \
  /tmp/soul-container-pure-probe/edge_chest/edge_chest_pure_dir.vpk \
  --oracle /tmp/soul-container-oracle/edge_chest/oracle.json \
  --force
```

Current result across `piplup`, `cinna`, `edge_chest`, `edge_cesium_man`, and
`edge_fox`: generated resources parse through `morphic::material::parse` /
`morphic::inspect`; VMAT shader, texture-param keys, int-param keys, and
vector-param keys match the CSDK oracle. Expected differences remain:

- Color texture paths are deterministic `<material>_color.vtex`, not CSDK's
  hash-suffixed `<material>_color_png_<hash>.vtex`.
- VTEX format is inline `PNG_RGBA8888`, not CSDK's BC7.
- Output has no default texture copies and no `.vmdl_c`.

In-game breakpoint as of the `pak43_dir.vpk` probe:

- Pure `PNG_RGBA8888` VTEX is accepted by the engine.
- Generated DATA-only VMAT failed red/wireframe.
- Generated `DATA+INSG` VMAT failed red/wireframe.
- Oracle `RERL/RED2/INSG` with fully regenerated v4 DATA failed red/wireframe.
- Oracle `RERL/RED2/INSG` and byte-faithful v5 DATA patched only at
  `g_tColor` rendered successfully.

This means the immediate VMAT replacement target is not a lossy full KV3
re-encode. The next pure backend should preserve or generate engine-compatible
v5 DATA layout, then update `g_tColor` byte-faithfully; after that, generate
matching `RERL` / `RED2` instead of borrowing oracle blocks.

Do not install a Rust-prep VPK just because graph parity looks good. Keep using
`soul_container_acceptance` and `v0sanity` as the gate; only install artifacts
that pass both and report embedded geometry, correct material refs, draw/material
counts, and largest bounds axis around `12.65`.

## Reverse Engineering Boundary

We can and should reverse engineer what the compiler emits enough to validate,
diff, patch, and eventually replace isolated pieces. The repo already decodes
large parts of `.vmdl_c`, `.vmat_c`, `.vtex_c`, KV3 blocks, vertex/index
buffers, meshopt-compressed buffers, textures, and material references.

Replacing `resourcecompiler.exe` for brand-new models is a much larger target.
For a GLB/FBX model compile it is doing at least:

- ModelDoc `.vmdl` parsing.
- FBX scene import, transform baking, triangulation, mesh splitting, and
  material slot binding.
- Source-relative material resolution and dependency graph generation.
- Texture import, format choice, mip generation, hash-suffixed `.vtex_c` naming,
  and compiled texture block emission.
- `.vmat_c` emission from source VMAT plus shader/static combo metadata.
- `.vmdl_c` resource block construction, including DATA/MDAT/VBIB-style geometry
  payloads, bounds, draw calls, material refs, physics data, and dependency
  metadata.
- Meshopt encoding and Source 2 resource header/block alignment details.

The pragmatic production path is therefore:

1. Use `resourcecompiler.exe` for brand-new model/material/texture resources.
2. Keep reverse-engineering the compiled output with small corpus tests.
3. Replace compiler substeps only after we can prove byte-accurate or
   engine-accepted output across multiple models, materials, and texture types.

Do not treat byte-identical output as the only success condition. In the local
probe, repeated successful compiler runs could produce different VPK hashes
while preserving the same resource paths, material refs, meshopt state, and
model bounds. Use hashes for installed artifact metadata; use resource
inspection for pipeline correctness.

## Resourcecompiler Launch

The CSDK compiler worked through Proton using:

```sh
STEAM_COMPAT_DATA_PATH=/tmp/proton-vpkmerge-rc \
STEAM_COMPAT_CLIENT_INSTALL_PATH=/home/esoc/.local/share/Steam \
SteamAppId=1422450 SteamGameId=1422450 VPROJECT=1 \
"/home/esoc/.local/share/Steam/steamapps/common/Proton - Experimental/proton" run resourcecompiler.exe \
  -game citadel -addon <addon> -fshallow -nop4 -v -consoleapp -consolelog -condebug -toconsole \
  -danger_mode_ignore_schema_mismatches \
  -filelist Z:\\tmp\\filelist.txt
```

Run from:

```text
/home/esoc/csdk12/Reduced_CSDK_12/game/bin_tools/win64
```

Important details:

- Use `-game citadel`, not an absolute `gameinfo.gi` path.
- Keep command-line options before input files/filelists.
- `-danger_mode_ignore_schema_mismatches` is currently required; otherwise the
  tool aborts on the `ParticleFloatType_t` schema mismatch before compiling.
- Absolute source paths under `content/citadel_addons/<addon>` compile to
  matching output under `game/citadel_addons/<addon>`.

## Required Content Layout

For an override VPK, source content should sit under:

```text
content/citadel_addons/<addon>/models/props_gameplay/soul_container/
  soul_container.vmdl
  model.fbx
  materials/
    <material>.vmat
    <texture>.png
```

The compiled output then lands under:

```text
game/citadel_addons/<addon>/models/props_gameplay/soul_container/
  soul_container.vmdl_c
  materials/
    <material>.vmat_c
    <texture>_png_<hash>.vtex_c
```

Resourcecompiler may also emit default dependency textures under
`materials/default/*.vtex_c`.

## Material Binding Trap

The critical fix is in the FBX material names.

Bad:

```text
cinna
```

This makes resourcecompiler search for `cinna.vmat`, which is an illegal/missing
resource path in the compiled model.

Good:

```text
models/props_gameplay/soul_container/materials/cinna
```

Resourcecompiler resolves that to:

```text
models/props_gameplay/soul_container/materials/cinna.vmat
```

The `material_search_path` field on `RenderMeshFile` did not fix this in the
probe. Source 2 material remaps exist, but naming the FBX materials as
Source-relative material paths is the simpler and more reliable import path.

## Scale And Origin Trap

Do not pass raw GLB dimensions straight through FBX. The first Piplup proof
compiled and loaded, but came out about 192x too large:

```text
span=[1716.2643, 1788.4462, 2432.7847]
```

The stock soul container bounds are:

```text
min=[-6.322958, -6.322958, -6.325]
max=[6.322958, 6.322958, 6.325]
span=[12.645916, 12.645916, 12.65]
```

Normalize imported geometry before FBX export by computing world-space mesh
bounds, moving the bounds center to the origin, and scaling the largest axis to
`12.65` Source units. In the Blender probe this used:

```text
scale = target_largest_axis / (imported_largest_axis * source_units_per_blender)
target_largest_axis = 12.65
source_units_per_blender = 100
```

Apply that transform to mesh vertices directly and export only mesh objects.
This avoids GLTF empties or FBX unit conversion preserving an oversized parent
transform.

## Proof Results

Cinna FBX probe:

- Compiled successfully with resourcecompiler.
- Output model referenced `models/props_gameplay/soul_container_fbxprobe/materials/cinna.vmat`.
- Emitted `.vmat_c` and `.vtex_c`.
- Vertex count matched the community Cinna model: `9711`.
- `m_bMeshoptCompressed = [true, true]`.

Piplup GLB probe:

- Input: `/home/esoc/Downloads/piplup.glb`.
- Output proof VPK: `/tmp/piplup_resourcecompiled_soul_container_dir.vpk`.
- VPK entries: `18`.
- Model entry: `models/props_gameplay/soul_container/soul_container.vmdl_c`.
- Draw calls/materials: `6`.
- Vertex count after center-and-fit normalization: `7158`.
- Bounds after normalization: `span=[8.924234, 9.299567, 12.65]`.
- `m_bMeshoptCompressed = [true, true]`.
- Resourcecompiler log: `OK: 1 compiled, 0 failed, 0 skipped`.

## Remaining Production Work

- Wire the maintained compiler wrapper into the app/Grimoire flow.
- Implement pure `.vmdl_c` emission for the constrained static soul-container
  path.
- Replace VMAT full DATA re-encode with byte-faithful v5 DATA construction or
  patching. The `pak43_dir.vpk` probe proves v5 DATA patched only at `g_tColor`
  renders in game.
- Generate matching `RERL` / `RED2` dependency/edit blocks for pure VMAT output
  instead of borrowing oracle blocks.
- Preserve more GLB material channels, not just base color.
- Decide whether to include resourcecompiler-emitted default textures or rely on
  base-game defaults when packing.
- Surface resourcecompiler logs in Grimoire so material/path failures are visible.
