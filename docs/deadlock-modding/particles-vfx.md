# Particles and VFX

A Deadlock ability effect is rarely one color field in one particle file. A
complete recolor or restyle can span particle graphs, textures, materials, and
model vertex colors.

## Inventory the effect graph

Start from the ability or hero data, resolve the named particle system, then walk
all child particle references. For every system, record:

- Color constants and gradients in `.vpcf_c`.
- Referenced sprite, mask, ramp, noise, and flipbook textures.
- Referenced materials and their tint or dynamic parameters.
- Rendered models and any baked vertex colors.
- Timing, lifetime, radius, roll, alpha, trail, and control-point inputs.

Directory prefixes are useful search hints, not ownership proof. Systems and
textures can be shared across heroes. Recoloring a shared entry in place can
change unrelated abilities.

## Common particle output fields

Observed `m_nOutputField` values used by float initializers and operators include:

| ID | Field |
|---:|---|
| 1 | Life duration |
| 3 | Radius, also a common default when absent |
| 4 | Roll |
| 5 | Roll speed |
| 6 | Color or tint |
| 7 | Alpha |
| 8 | Creation time |
| 9 | Sequence number |
| 10 | Trail length |

Patch the input value owned by the matching operator. Do not globally replace a
number because the same literal can occur in unrelated fields or child systems.
Curve-driven and control-point-driven inputs may not have a literal value to edit.

## Preserve sprite shape

Many particle textures are shape fields, not pictures. Edge, mask, smoke, soft,
beam, flame-shape, and falloff textures often rely on luminance or alpha to make
the sprite disappear at its borders.

For a recolor:

- Preserve luminance and alpha unless the shader says otherwise.
- Move hue and saturation rather than stamping a flat rectangle.
- Do not scroll a one-axis falloff across its non-tiling axis.
- Treat flipbook and sheet metadata as part of the texture contract.

A square or hard seam usually indicates destroyed falloff data, an inappropriate
scroll, or a wrong texture class, not necessarily a broken particle renderer.

## Repoint instead of globally replacing shared assets

If a texture or material is shared, create a hero-specific copy and rewrite only
the target particle's reference. The replacement should match the original class:
a flipbook should remain a flipbook, and a sheet texture should keep compatible
sequence metadata.

This produces a larger mod than overriding a shared texture, but it avoids
changing every system that consumes that resource.

## Animation edits

Particle animation can be driven by authored time inputs, gradients, texture
offsets, and particle age. Preserve the original sign and scale of multipliers
when increasing an effect. Replacing an authored `-0.1` with a large positive
constant can invert behavior and magnify it at the same time.

When adding a rainbow or cycle, start with a static recolor. Add one timing change
at a time and compare the resulting graph. This isolates color correctness from
animation correctness.

## Byte-faithful editing

Full KV3 re-encoding has produced engine failures such as red X particles even
when the decoded logical tree looked valid. Prefer targeted scalar, array, or
blob patches that preserve unknown data and compression structure.

If a full encode is unavoidable:

1. Validate with an independent parser.
2. Diff resource blocks and dependencies.
3. Load a one-file probe in the game before baking a whole hero or roster.

## VFX validation checklist

- Test the stock effect and the mod from the same game build.
- Disable stale addons that override any child entry.
- Check cast, loop, impact, lingering, and cleanup phases.
- View the effect against light and dark backgrounds.
- Check ally/enemy, first-person/world, and picker/gameplay variants when present.
- Rebuild recipe-based mods after updates and confirm every pinned path still
  exists.

Related: [Rainbow Prism QA](../rainbow-prism-qa-findings.md),
[vertex-color recolor](../handoff-vertex-color-recolor.md), and
[customization frontier](../customization-frontier.md).
