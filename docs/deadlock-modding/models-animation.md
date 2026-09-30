# Models and animation

Compiled Deadlock models combine geometry, draw-call groupings, skeleton data,
attachments, physics, animation references, and external resource dependencies.
A visually simple mesh replacement can depend on all of them.

## Export is not the same as round-trip editing

glTF is a good interchange and preview format, but it does not preserve every
Source 2 model detail. In particular:

- Axis conversion and unit scale can live in glTF node matrices.
- A consumer that reads raw accessors but ignores node transforms will interpret
  Blender-exported positions in the wrong space.
- Skin joint indices are local to each glTF skin and must be mapped by bone name,
  not assumed to match another model's numeric order.
- Material, physics, attachment, and engine-specific block semantics can be lost.

When grafting compatible geometry, direct buffer operations can be safer than a
Blender round-trip. If Blender is required, bake the intended transforms and
verify the importer contract explicitly.

## Skeleton compatibility has two dimensions

Matching bone names are not sufficient. A replacement must consider:

1. **Bone coverage**: every weighted source bone must map to a target bone or be
   deliberately merged into a safe parent.
2. **Rest pose compatibility**: two heroes can share names but have different
   shoulder, hand, spine, or leg rest transforms.

The shipped roster largely shares a humanoid core, but only a small subset has
near drop-in rest-pose compatibility. Measure candidate donors from the current
game build before choosing one.

Map joints case-insensitively by name, report every missing weighted bone, and
measure the target's proportions instead of assuming one global scale.

## Geometry replacement rules

- Preserve or rebuild all required vertex attributes: positions, normals,
  tangents, UVs, joint indices, weights, and colors.
- Recalculate indices after concatenating primitives.
- Normalize weights and ensure every vertex has a valid target binding.
- Patch every relevant LOD. Editing only the nearest LOD causes a visible snap at
  distance.
- If collapsing or removing geometry, rebind all weight lanes to the intended
  bone. Moving positions alone can let animation reopen the hidden geometry.
- Keep draw-call groups and material assignments coherent. A multi-material
  character should not be flattened into one material without an explicit atlas
  and shader plan.

## Mesh compression warning

The repository has observed model replacements that decode correctly offline but
render as exploded geometry when re-emitted with the current meshopt encoder.
The proven safe path for edited buffers is to store them uncompressed and mark
the mesh accordingly. Do not preserve a compressed flag around newly uncompressed
bytes, and do not claim engine compatibility from an offline decode alone.

## Attachments, cameras, and equipment

Camera framing and attachment behavior can live in model data rather than the
mesh or skeleton alone. A replacement that animates correctly may still have a
bad over-shoulder camera or misplaced equipment. Inventory attachment records and
model-data blocks before concluding that a skeleton swap is complete.

## Cloth and secondary motion

Some secondary motion is not a set of ordinary animation bones. Deadlock models
can carry finite-element data in physics blocks. Static pose export can freeze or
distort these nodes when the chosen clip drives only part of the mesh skeleton.

Treat cloth as a separate verification area:

- Check whether the source uses finite-element data, jiggle bones, or authored
  animation.
- Test more than one pose.
- Verify the model in motion, not only in a still GLB render.
- Expect ModelDoc or ResourceCompiler authoring to be required for some new cloth
  setups.

## Model validation checklist

1. Export the original and replacement with the same tool version.
2. Compare bone names, weighted-bone coverage, rest transforms, and proportions.
3. Check every primitive and LOD for finite values and valid index bounds.
4. Render an offline preview with animation.
5. Reopen the packed model and compare block/dependency inventories.
6. Test camera, attachments, equipment, outline shells, cloth, and LOD changes
   in-engine.

Related: [model exporter](../vmdl-glb-exporter.md),
[model editing handoff](../handoff-model-edit.md),
[animation authoring](../anim-authoring-pipeline.md), and
[loose clip posing](../handoff-nm-loose-clip-pose.md).
