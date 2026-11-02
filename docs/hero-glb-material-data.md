# Hero GLB material data

Material `extras.morphic.schema_version` is now **3**. The texture metadata
reader also exposes the four VTEX header floats as `TextureInfo.reflectivity`.
They are linear values copied from the compiled texture, not averages computed
from exported pixels.

For `pbr.vfx`, the textured exporter writes:

| Source | GLB destination |
| --- | --- |
| Opaque color alpha | Metallic-roughness B, with `metallicFactor = 1` |
| Normal texture B | Metallic-roughness G, with `roughnessFactor = 1` |
| Color VTEX reflectivity | `extras.morphic.texture_reflectivity.g_tColor`, four floats |
| Ambient occlusion R | `occlusionTexture` R, including constant-sized textures |

Alpha-tested, translucent, advanced-translucency, and additive materials retain
color alpha as opacity. Their color alpha never becomes metalness. Separate
metalness textures and constant factors retain their existing fallback behavior.
Opaque base-color alpha remains 255.

The `pbr.vfx` roughness rule overrides both the pure-normal heuristic and a bound
standalone roughness texture. It includes constant normal textures. Other
shaders retain the heuristic and standalone-roughness priority. Packed ORM uses
the maximum source width and height, nearest-neighbor sampling the smaller
image, so a lower-resolution normal does not discard higher-resolution
metalness texels.

The AO exception adds images whose **both axes are at most 4**. That adds at
most 16 texels or 64 decoded RGBA bytes per constant texture, plus PNG/container
overhead. Ordinary AO images with both axes above 4 retain their existing
behavior. Thin images previously rejected by the min-axis filter stay rejected.
Emissive placeholder filtering is unchanged.

## Grimoire integration

Update the helper binary or released helper pin, then bump
`SOURCE2_EXTRAS_VERSION` in `src/lib/source2ExtrasVersion.ts` from **2 to 3**.
That matches this exporter's per-material schema version.

`electron/main/services/heroPoseModels.ts` incorporates that shared version in
both `POSE_CACHE_VERSION` and `RIGGED_CACHE_VERSION`. On the inspected Grimoire
checkout, this changes `16.x2` to `16.x3` and `20.x2` to `20.x3`, respectively.
Its `.cache-version` and `.rigged-cache-version` checks then regenerate old
GLBs. Changing the exporter schema alone does not invalidate Grimoire's files.
No separate pipeline-version bump is needed for this material schema change.

The renderer already reads `texture_reflectivity.g_tColor` in
`source2NprMaterial.ts` and uses the first three components as its linear color
correction pivot. Metallic/roughness channels and factors arrive through the
ordinary glTF material. No Grimoire files are modified by this change.

## Verification

Unit tests write and read complete GLBs with synthetic compiled textures and
materials. They cover metalness/roughness packing, a normal that fools the
heuristic, both source-resolution orderings, constant AO, exact header
reflectivity, the opacity exceptions, and the other-shader roughness path.

The installed Yamato model also needs compact KV3 unsigned constants, tags 26
and 27. The separate decoder fix has a regression test for these payload-free
values; the shipped tool has no .NET dependency.

## Verification

The exported channels were checked against VRF-decoded source textures for five
heroes, reading the installed `game/citadel/pak01_dir.vpk` in place. Source
texels were decoded with `TextureCodec.None` and embedded GLB PNGs were read
without premultiplication. Reflectivity is compared by all four f32 bit
patterns. AO is compared at normalized texture coordinates because VRF crops
the default texture to its real 1x1 dimensions while the exporter retains its
stored 4x4 pixels. Each model was exported with
`model export --vpk <pak01_dir.vpk> --entry <model-entry> --no-anim`; static
and rigged exports share the material-writing path.

| Hero | Model entry | Materials | Metal / rough / opacity differences | Reflectivity exact | AO exact |
| --- | --- | ---: | --- | --- | --- |
| Wraith | `models/heroes_wip/wraith/wraith.vmdl_c` | 6 | 0 / 0 / 0 | 6/6 | 6/6 |
| Yamato | `models/heroes_staging/yamato_v2/yamato.vmdl_c` | 2 | 0 / 0 / 0 | 2/2 | 2/2 |
| Seven | `models/heroes_staging/gigawatt_prisoner/gigawatt_prisoner.vmdl_c` | 4 | 0 / 0 / 0 | 4/4 | 4/4 |
| McGinnis | `models/heroes_wip/mcginnis/mcginnis.vmdl_c` | 9 | 0 / 0 / 0 | 9/9 | 9/9 |
| Abrams | `models/heroes_wip/abrams/abrams.vmdl_c` | 7 | 0 / 0 / 0 | 7/7 | 7/7 |

All 28 materials use `pbr.vfx` with `F_USE_NPR_LIGHTING = 1`. Every ORM texel
was checked with nearest resampling at differing resolutions; two materials
exercise real opacity exceptions. These checks validate the data in the
exported GLBs; they do not measure Grimoire rendering error or in-game
appearance.
