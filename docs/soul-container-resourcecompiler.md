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
- Preserve more GLB material channels, not just base color.
- Decide whether to include resourcecompiler-emitted default textures or rely on
  base-game defaults when packing.
- Surface resourcecompiler logs in Grimoire so material/path failures are visible.
