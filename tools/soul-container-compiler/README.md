# Soul Container Compiler Tool

Builds a real Source 2-compiled soul-container override VPK from a custom GLB.

This is a dev-time Linux/Proton wrapper around Valve's `resourcecompiler.exe`.
It does not try to hand-write `.vmdl_c`, `.vmat_c`, or `.vtex_c`; it generates
source content and lets the Source 2 compiler produce the compiled resources.

## Prerequisites

- Blender on `PATH`, or set `BLENDER=/path/to/blender`.
- Deadlock CSDK extracted at `/home/esoc/csdk12/Reduced_CSDK_12`, or set
  `CSDK_ROOT=/path/to/Reduced_CSDK_12`.
- Proton at
  `/home/esoc/.local/share/Steam/steamapps/common/Proton - Experimental/proton`,
  or set `PROTON=/path/to/proton`.
- Steam root at `/home/esoc/.local/share/Steam`, or set
  `STEAM_ROOT=/path/to/Steam`.

## Build A VPK

From the repo root:

```sh
tools/soul-container-compiler/build_soul_container.py \
  /home/esoc/Downloads/piplup.glb \
  --addon piplup_probe \
  --output /tmp/piplup_resourcecompiled_soul_container_dir.vpk \
  --force
```

The pipeline is:

```text
GLB
-> Blender import, center, scale, FBX export, VMAT/PNG/VMDL source generation
-> resourcecompiler.exe through Proton
-> game/citadel_addons/<addon> compiled output
-> pack_tree dir VPK
```

## Install Into A Specific Addon Slot

```sh
tools/soul-container-compiler/build_soul_container.py \
  /home/esoc/Downloads/piplup.glb \
  --addon piplup_probe \
  --output /tmp/piplup_resourcecompiled_soul_container_dir.vpk \
  --install-to /home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/addons/pak06_dir.vpk \
  --metadata /home/esoc/.config/grimoire/mod-metadata.json \
  --force
```

When `--metadata` is passed, the tool updates only the installed VPK's SHA in
Grimoire metadata. It preserves any existing title and thumbnail for that VPK
key.

## Scale Defaults

The stock soul container's largest axis is `12.65` Source units. The Blender
stage recenters imported world-space mesh bounds at the origin and scales the
largest axis to that target after accounting for the observed `100x` FBX to
Source unit conversion:

```text
target_largest_axis = 12.65
source_units_per_blender = 100
```

Override with `--target-largest-axis` or `--source-units-per-blender` only after
checking the compiled model bounds in game or with a model inspection tool.

## Material Limits

The current Blender stage preserves material slots and base-color textures. It
does not yet map every GLB PBR channel into a Deadlock-grade material stack.
That is intentionally separate from the compiler path: the important bit is
that material names in the FBX are Source-relative VMAT paths before
`resourcecompiler.exe` runs.
