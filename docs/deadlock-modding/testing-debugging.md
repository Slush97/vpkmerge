# Testing and debugging Deadlock mods

Mod validation should progress from cheap structural checks to a clean in-game
test. Skipping directly to a large merged addon makes path, format, collision,
and content errors difficult to distinguish.

## Validation ladder

1. **Parse the source** with the primary tool.
2. **Parse it independently** when an oracle exists.
3. **Apply one small edit** and reopen the output.
4. **Diff logical values and resource blocks** against the source.
5. **Pack one override** at the original virtual path.
6. **Reopen the packed VPK** and inspect that exact entry.
7. **Mount it alone** in a free addon slot.
8. **Exercise every relevant gameplay phase** in retail Deadlock.
9. **Add the result and game-build context** to the durable docs.

Use offline acceptance as a gate, not as the final verdict.

## Manual addon isolation

In the verified local setup, manual test addons live under:

```text
Deadlock/game/citadel/addons/pak0N_dir.vpk
```

Use a free `N`, disable other addons that override the same paths, and restart the
game when testing mount behavior. Move inactive builds out of the active directory
or into an `.disabled/` directory.

Never replace the base `citadel/pak01_dir.vpk` index with a mod.

## Reload versus remount

Not every resource class reloads the same way:

- Loose textures and materials under a registered search path can often be
  refreshed with `mat_reloadallmaterials`.
- Particle graphs, models, and archive membership commonly need a map change or
  game restart.
- Retail Deadlock has historically ignored `-netconport`, so do not design a
  critical workflow around a TCP console without retesting the current build.

Use a one-key in-game bind for reload commands when socket automation is
unavailable.

## Diagnose by symptom

| Symptom | First checks |
|---|---|
| Mod has no effect | Virtual path, addon mount, collision priority, stale build |
| Pink or missing texture | Referenced path, texture class, compiler transform, format/dimensions |
| Red material | Full material re-encode, bad KV3 compression/blob layout, missing dependency |
| Red X particle | Particle KV3 re-encode, missing child, incompatible resource version |
| Square particle | Destroyed sprite falloff/alpha, non-tiling texture scroll |
| Exploded model | Mesh compression, index bounds, joint remap, node transform |
| Model disappears | Unit scale, ignored glTF node matrix, wrong draw-call group |
| Audio plays intermittently | Randomizer pool not fully replaced |
| Audio affects every hero | Shared clip overridden in place |

## Surviving game updates

Recipe-based mods are snapshots of asset paths and assumptions. After an update:

1. Save a pre-update VPK path and CRC inventory when possible.
2. Identify changed `pak01_*.vpk` chunks by modification time.
3. Map changed chunks back to virtual paths.
4. Diff CRCs, dependencies, and relevant KV3 fields.
5. Verify every pinned recipe path still exists.
6. Check for hero namespace migrations and newly shared resources.
7. Rebuild from the new base assets. Do not keep overriding a changed base file
   with an old compiled copy unless that is intentional.
8. Test the rebuilt addon in isolation.

An apparently broken current feature can be a stale addon built from an older
base resource rather than a defect in the current editing code.

## CSDK and community-tool builds

Treat community-packaged Source 2 authoring kits as build-specific. A tools engine
from one Deadlock snapshot may reject newer particle GUIDs, sound-mix versions, or
other compiled formats. Feeding current retail archives to an older tools build
can crash during startup.

Record the content and tool build dates together. Use the retail game as the final
loader for mods targeting retail, even when ModelDoc, Particle Editor, or
ResourceCompiler is used for authoring.

## A useful bug report

Include:

- Deadlock build date or manifest context.
- Tool commit or release.
- Input virtual path and source archive.
- Exact command and output path.
- Whether the output passes primary and independent parsers.
- Active addon collision inventory.
- In-game symptom, map, hero, and reproduction phase.
- The smallest mod that still reproduces the issue.
