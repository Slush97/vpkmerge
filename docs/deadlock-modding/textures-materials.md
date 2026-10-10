# Textures and materials

Deadlock's character look is split across texture pixels, material parameters,
vertex colors, and lighting. A texture-only investigation can therefore reach
the wrong conclusion even when every texture was decoded correctly.

## Trace the full material stack

For a model part, record at least:

1. The `.vmat_c` selected by the draw call.
2. The material's shader and feature flags.
3. Every referenced `.vtex_c` slot.
4. Vertex color use, especially `F_VERTEX_COLOR`.
5. Dynamic material expressions and runtime attributes.

A flat 4x4 texture is often an intentional constant, not missing detail. Some
visible color can be entirely vertex-painted.

## Common hero texture packing

For Deadlock hero materials using `pbr.vfx`, observed
`g_tNormalRoughness` textures commonly use:

- R and G: normal X and Y.
- B: roughness.
- A: often unused or constant.

Do not assume a metalness channel exists. A metal-like look can be authored with
a metal-colored albedo and low roughness. Inspect the actual shader features and
texture statistics before assigning channel semantics.

When changing only roughness, preserve the normal channels. Write data textures
without a color-space conversion so byte values do not shift.

## Preserve masks and alpha

Alpha is frequently a shader mask rather than opacity. Examples include
alpha-test silhouettes, self-illumination control, and other material-specific
branches. Preserve source alpha unless the material proves that alpha is the
intended edit target.

Preview software may premultiply RGB by alpha and show an opaque in-game texture
as black or transparent. Configure the preview to ignore alpha when validating
RGB-only usage.

## Recolor without flattening the art

Deadlock's NPR read depends heavily on authored luminance separation. A global
desaturate-and-tint pass can erase cloth, skin, hardware, and shadow structure.

A robust recolor pipeline:

1. Preserve each texel's original luminance.
2. Move hue and saturation with soft region weights.
3. Desaturate toward gray when the target color clips, instead of changing
   luminance.
4. Treat emissive accents separately because a saturated color cannot preserve
   arbitrarily high luminance.
5. Leave black fields black on `F_UNLIT` materials unless permanent fullbright
   output is intended.

Keep shape-bearing grayscale textures shape-bearing. Replacing a feathered beam,
smoke, edge, or mask with a flat color rectangle creates square particles even
when the particle graph is untouched.

## UV overlap changes the authoring workflow

Hero meshes commonly use mirrored or overlapping UV islands. Baking a procedural
material from the mesh back into the original UV map can cause surfaces to fight
for the same texels and produce dense artifacts.

For ordinary reskins, author in image space using the original texture layout.
Composite new color or pattern information over the source image so the painted
detail survives. Use mesh-based mask baking only when you have separated the
selected faces or otherwise prevented overlapping surfaces from writing to the
same target.

## Region signals when no tint mask exists

Many hero bodies do not provide a useful spatial tint mask. Other shipped maps
can still guide broad segmentation:

- Albedo separates painted color families.
- Ambient occlusion local contrast often separates hard-surface fittings from
  smoother cloth.
- Self-illumination masks identify the regions designed to glow.
- Vertex colors can carry gradients that do not exist in any texture.

These signals are useful for broad procedural regions. They cannot uniquely
identify overlapping UV islands. Precise part selection still requires geometry
or a face-derived mask.

## Material edits

`pbr.vfx` exposes artist-set UV scale, offset, and scroll parameters. Non-zero
albedo or self-illumination scroll can animate a tileable texture without editing
the shader. There is no observed artist-set switch for a true world-locked or
screen-locked projection on this shader.

Dynamic parameters are compiled expressions. Existing expressions can be
decompiled and replaced through blob-aware patching, but local-variable and
multi-statement programs may exceed the supported expression grammar.

Avoid full material reserialization when an in-place scalar or blob patch is
possible. Offline structural validity is necessary but does not guarantee the
engine will accept a rebuilt material.

## Texture validation checklist

- Confirm the source texture format, stored dimensions, and actual dimensions.
- Apply required `RED2` transforms before judging decoded pixels.
- Preserve alpha and non-target channels.
- Confirm the replacement has compatible dimensions, mip count, and format.
- Reopen the packed VPK and decode the exact packed entry.
- Inspect every active addon that overrides the same virtual path.
- Test close-up, at distance, and under more than one lighting condition.

Related: [Deadlock skin texture findings](../findings-deadlock-skin-textures.md),
[blobbed material patching](../spike-blobbed-vmat-recolor.md), and
[NPR shading research](../spike-npr-toon-shading.md).
