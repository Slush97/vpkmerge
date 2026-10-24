# Workshop materials and movement effects

These commands build small override VPKs from the installed game's compiled resources. They do not edit the input archive.

## Material finishes

```sh
vpkmerge workshop finish --vpk pak01_dir.vpk --entry materials/example.vmat_c \
  --roughness 0.18 --metalness 1 --encode-vpk finish_dir.vpk
```

The command supports the material's existing roughness/metalness bindings. Packed normal-map masks are cloned before modification so other materials keep their original texture. Unsupported bindings fail instead of silently changing unrelated channels.

## Movement effects

```sh
vpkmerge workshop movement --vpk pak01_dir.vpk --kind dash --color '#ff70cf' \
  --end-color '#65dfff' --encode-vpk dash_dir.vpk
vpkmerge workshop movement --vpk pak01_dir.vpk --kind dash --color '#ffffff' \
  --image stamp.png --illuminated --outline-color '#ffffff' --encode-vpk stamps_dir.vpk
vpkmerge workshop movement --vpk pak01_dir.vpk --kind heavy-melee --color '#58d6ff' \
  --encode-vpk melee_dir.vpk
```

`--rainbow` replaces `--end-color`. Dash trails also accept `--brightness` from 0.25 to 4. Images must be PNGs up to 512 pixels per side and 4 MiB. White preserves their colors; other colors tint them.

Image trails replace the rope renderer with sprites. The compiled particle's external resource list and texture hash are updated together with its renderer reference. The stamp uses a game-authored BC7 sprite layout, including its mip chain and transparency.

The particle graph is bounded to 256 systems. Recoloring preserves unedited fields, alpha, placement and fade operators. Gradient and rainbow modes add color operators to the compiled graph.

## Preview motion bundles

```sh
vpkmerge workshop motions --vpk pak01_dir.vpk --entry models/example.vmdl_c \
  --clip idle=models/example/idle.vnmclip_c \
  --clip dash=models/example/dash.vnmclip_c --out motions.glb
```

Repeated `--vpk` arguments resolve resources in priority order. `--clip NAME=legacy:CLIP_NAME` selects an embedded legacy animation. Bundles contain animation and skeleton data without meshes. Root translation is removed for in-place playback; authored rotation and scale remain intact.

Additive clips need an explicit base and mask and are rejected here. The caller can blend compatible full-body clips. This does not reconstruct Valve's gameplay animation graph or establish in-game particle parity.
