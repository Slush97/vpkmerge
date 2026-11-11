# Source 2 preview data contract

This document is the durable contract between the Source 2 decoder/exporter (morphic) and the three.js hero preview that consumes its output: the FeModel cloth binary layout, the GLB schema-v2 `extras` payload, and the per-flag disposition of the `pbr.vfx` uber-shader. For the higher-level overview of the decoder, the GLB writer, the `vmat_style` material editor, and the `vfx_expr` expression codec, see [CLAUDE.md](../CLAUDE.md).

## 1. FeModel / PHYS field map

Source 2 cloth simulation parameters are not opaque. The cloth solver data lives in a model's PHYS block as `m_feModel`, encoded as binary KV3 (no resource envelope; decode the raw block bytes directly through the KV3 codec). The block is located via the resource block table, and `m_feModel` may sit at the document root or nested under `m_parts[*]`.

The data describes a Position-Based Dynamics (XPBD) setup, not heavy FEM: rods are distance constraints, `flRelaxationFactor` is stiffness, and inverse mass plus iteration count are standard XPBD inputs. The fields map onto a verlet/XPBD loop.

### Field map

The decoder reads the following fields. The "example" column shows one shipped hero's authored values to indicate typical magnitudes and counts.

| Solver concept | FeModel field | Type / shape | Notes (example values) |
|---|---|---|---|
| node count | `m_nNodeCount` | int | 539 total |
| static node count | `m_nStaticNodes` | int | 86 static (pinned) |
| control names | `m_CtrlName` | string[] | one per node, maps node to bone (e.g. `$cloth_m0p33`) |
| inverse mass | `m_NodeInvMasses[]` | f32[] | `0` means pinned/static |
| distance constraints | `m_Rods[]` | struct[] | each `{nNode, flMinDist, flMaxDist, flWeight0, flRelaxationFactor}` |
| SIMD constraint mirror | `m_SimdRods[]` | struct[] | f4-packed duplicate of `m_Rods` (redundant) |
| stiffness | `m_Rods[].flRelaxationFactor` (+ `flWeight0`) | f32 | per-constraint |
| stretch/compress limits | `m_Rods[].flMinDist` / `flMaxDist` | f32 | per-constraint |
| per-node integrator | `m_NodeIntegrator[]` | struct[] | each `{flPointDamping, flAnimationForceAttraction, flAnimationVertexAttraction, flGravity}` |
| damping | `m_NodeIntegrator[].flPointDamping` | f32 | per-node |
| gravity | `m_NodeIntegrator[].flGravity` | f32 | per-node |
| pull-back-to-pose | `flAnimationForceAttraction`, `flAnimationVertexAttraction` | f32 | how hard cloth tracks the animated rest shape |
| collision radius | `m_NodeCollisionRadii[]` | f32[] | indexed by dynamic slot (see binding conventions) |
| friction | `m_DynNodeFriction[]` | f32[] | indexed by dynamic slot |
| collision capsules | `m_TaperedCapsuleRigids[]` | struct[] | each `{vSphere, nNode, nCollisionMask, nVertexMapIndex, nFlags}`; pre-placed and bone-pinned |
| sphere rigids | `m_SphereRigids[]` | struct[] | `FeSphere` shape |
| box rigids | `m_TaperedCapsuleRigids` siblings / box variant | struct[] | `FeBox` shape |
| ropes / chains | `m_Ropes`, `m_nRopeCount` | int / struct[] | independent cloth chains |
| node frames | `m_NodeBases[]` / `m_SimdNodeBases[]` | struct[] | each `{nNode, nNodeX0..Y1, qAdjust}` |
| control-to-node maps | `m_CtrlOffsets[]`, `m_CtrlSoftOffsets[]`, `m_ReverseOffsets[]` | struct[] | binding offsets (see conventions) |
| solver iterations | `m_nExtraIterations`, `m_nExtraGoalIterations` | int | engine defaults when both `0` |
| init pose | `m_InitPose[]` | struct[] | per-node `[x, y, z, 1, qx, qy, qz, qw]` (position then quaternion) |
| collision BVH | `m_TreeParents`, `m_TreeChildren`, `m_TreeCollisionMasks` | arrays | broadphase tree; carries per-node masks (see conventions) |
| world collision radius | `add_world_collision_radius` | f32 | folded by the decoder |
| default gravity scale | `default_gravity_scale` | f32 | folded by the decoder |

Constraint types that are present-but-empty on validated heroes (other heroes may populate them; their element shapes are unverified until a shipping hero uses them): `m_Quads`, `m_Tris`, `m_Twists`, `m_HingeLimits`, `m_KelagerBends`, `m_AxialEdges`, `m_CollisionPlanes`, `m_FitMatrices`, `m_WorldCollisionParams`, `m_JiggleBones`.

### Validated binding conventions

These conventions are validated to floating-point precision against multiple heroes by the decoder's rest-pose oracle. A re-implementer must match them exactly.

1. **Dynamic-slot fold.** `m_NodeCollisionRadii` and `m_DynNodeFriction` are indexed by the k-th node with `inv_mass > 0` (static nodes are excluded), not by raw node index.

2. **Per-node collision mask.** The mask lives in the collision BVH leaves, not in a dedicated per-node array. `m_TreeCollisionMasks` has length `2*D - 1` where `D` is the dynamic node count; leaves occupy tree indices `[0, D)`, and leaf k corresponds to the k-th dynamic node, so `mask[dynNode k] = m_TreeCollisionMasks[k]`. Apply this fold only when `len == 2*D - 1`; otherwise fall back to `0xFFFF` (collide-all). Validation: folded masks cleanly separate anatomical layers (hair, backpack, accessories, cloth sheet) with no intra-family conflicts.

3. **Position binding (exact at rest).** A child node's rest position is `child_init_pos == parent_init_pos + rotate(parent_init_rot, sign * offset)`, where `offset` is expressed in the parent's local frame and `sign` is recovered per-link (the choice of `+`/`-` that best reproduces the rest position). This applies to `m_CtrlOffsets` (parent to child), `m_ReverseOffsets` (target node to bone control, using the bone control's rest rotation), and `m_CtrlSoftOffsets` (same as reverse offsets, blended by `flAlpha`).

4. **Orientation (absolute).** A node's rest/solved world rotation is `basis(positions) * qAdjust`. The basis is built from node-frame seed indices as follows (Hamilton quaternions, `[x, y, z, w]` order):
   ```
   x_seed = normalize(pos[x1] - pos[x0])      // x1 first
   Y      = normalize(pos[y1] - pos[y0])       // primary axis
   Z      = normalize(cross(x_seed, Y))
   X      = normalize(cross(Y, Z))
   bone_world_rot = normalize(quatFromBasis([X, Y, Z]) * qAdjust)
   ```
   `qAdjust` is an absolute multiplier, not a relative delta. There is no rest-adjusted-inverse or rest-world-quaternion conjugation step. At rest this reproduces the stored `init_rot` exactly (the writeback is a no-op at rest), so solved writeback orientation is simply `basis(solved) * qAdjust`.

5. **Pinned nodes and units.** A node is pinned when `inv_mass <= 0`. `m_InitPose[i]` is `[x, y, z, 1, qx, qy, qz, qw]` (position then quaternion). Units are centimeters (Hammer), Z-up, with gravity near 360.

### Named approximations

These are deliberate approximations for a skinned bone-driven preview, not omissions:

- `flAnimationVertexAttraction` has no bone-level consumer in a skinned preview (it is per-vertex).
- The SIMD duplicate arrays (`m_SimdRods`, `m_SimdNodeBases`) are redundant with their scalar counterparts.
- The BVH tree's broadphase role is replaceable by brute-force collision (identical results at preview scale).
- World/ground collision is unused (a preview has no floor).

## 2. GLB schema-v2 extras contract

The GLB exporter bakes a Source 2 material payload onto each GLB material under `material.userData.morphic`. Standard PBR data is baked into native glTF channels (baseColorFactor including the color tint, `KHR_materials_emissive_strength`, `KHR_materials_sheen`, glass transmission/ior, unlit, and ORM metalness/roughness); the `morphic` payload carries the Source 2 material vocabulary the renderer needs on top of those channels.

### Extras keys

| Key | Type | Meaning |
|---|---|---|
| `schema_version` | int | Contract version. The renderer logs it per loaded material and uses it to detect and invalidate stale GLB caches. |
| `shader` | string | Source 2 shader name (e.g. `pbr.vfx`). |
| `blend_mode` | string/int | Derived blend/depth state for the material. |
| `flags` (or `ints`) | map | All Source 2 int params, keyed by Source 2 name. |
| `floats` | map | All Source 2 float params, keyed by Source 2 name. |
| `vectors` | map | All Source 2 vector params as `[x, y, z, w]`, keyed by Source 2 name. |
| `textures` | map | Resolved texture slots as glTF texture indices, keyed by Source 2 slot name. |
| `texture_slots` | list | The Source 2 texture-slot identities the material binds (slot name plus UV selection), so a slot's Source 2 identity survives even when it also maps to a standard glTF binding. |
| `dynamic_params` | map | Per-param dynamic-expression metadata from `m_dynamicParams` (see below). |
| `dynamic_texture_params` | map | Per-param dynamic-expression metadata from `m_dynamicTextureParams`. |
| `render_attributes_used` | list | Render attributes the material's expressions reference. |
| `texture_transforms` | map | Per-slot UV transform params: offset, scale, rotation, scroll speed, scroll quantize, model-scale axes and origins. |
| `uv_routing` | map | Which texture slots use the secondary UV set, derived from the `g_bUseSecondaryUvFor*` flags and `F_SECONDARY_UV`. |
| `vertex_inputs` | map | Vertex-stream requirements: `uses_vertex_color`, `uses_paint_vertex_colors`, `uses_tangent`, `has_tangent`, `has_color_0`, `uv_count`, `uses_secondary_uv`, `uses_jitter_vertices`. |
| `source2_summary` | object | Shader-vocabulary metadata for debugging. |

A material that has a dynamic override on a static param must be exposed as expression-driven so the renderer never silently trusts the static value. Each dynamic-param entry carries `source` (decompiled expression text), `decompiled` (bool), `byte_len`, and `attributes` (render attributes used). When a blob fails to decompile, the entry instead carries the param name, table name, byte length, a hex or stable hash of the bytecode, and the error text.

### Texture-slot list

The exporter resolves the full Source 2 texture-slot set the renderer can address by Source 2 slot name. Resolving these by slot name (rather than relying only on standard glTF bindings) preserves UV selection, slot identity, and strength params that the Source 2 shader approximation needs, even for slots such as AO that also map to a native glTF texture.

```
g_tColor              g_tColor1
g_tNormalRoughness    g_tNormalRoughness1
g_tPacked1            g_tMetalness
g_tAmbientOcclusion   g_tMasks1
g_tDetail             g_tTintMask
g_tTintMaskRimLightMask
g_tNprOutlineMask     g_tNprTransmissiveColor
g_tSelfIllumMask      g_tSheen
g_tGlass              g_tAltTranslucency
g_tJitterMask         g_tAltColor_Reveal
```

`g_tAltColor_Reveal` is `pbr.vfx`'s alt texture: a second albedo in RGB with a reveal mask in alpha. Every `pbr.vfx` material binds it (most to a default, or to their own color texture again), so it is embedded only when the material can show it: a static `g_flAltTextureAmount` above zero, or a dynamic expression on it. The pixel shader, decompiled from the shipped `pbr.vfx` (VCS 72), reads:

```
if (g_flAltTextureAmount > 0) {
    alt    = texture(g_tAltColor_Reveal, uv)        // the g_tColor UV
    reveal = g_flAltTextureAmount == 1 ? 1
           : smoothstep(-soft, soft, amount - (clamp(alt.a, 1e-4, 0.9999) * (1 - 2 * soft) + soft))
    albedo = mix(albedo, alt.rgb * vertexColorTint, reveal)
}
```

`soft` is `g_flAltTextureRevealSoftness`. `albedo` is the color texture after its color correction, tint and vertex color; the alt color skips the correction and the tint. Violet's brush (`artist_weapon.vmat`) drives the amount with `max($AmmoPercent, smoothstep(0.5, 1, $ReloadProgress))`, so a full magazine shows the inked texture everywhere.

Placeholder textures are preserved where the shader semantics require them. A solid-white default mask is the "present, full coverage" sentinel (for example a white self-illum mask means uniform coverage, not "off"); a solid-black default mask is the "absent" sentinel (for example a black transmissive texture contributes nothing).

## 3. pbr.vfx flag disposition table

Deadlock hero materials render almost entirely through one uber-shader, `pbr.vfx`. The table records each `F_*` flag's disposition in the preview renderer and what completing that flag requires.

| Flag | Disposition | Completion requirement |
|---|---|---|
| `F_ADDITIVE_BLEND` | partial | Validate additive blend and depth state. |
| `F_ADVANCED_TRANSLUCENCY` | partial | Translucency and alt-translucency approximation. |
| `F_ALPHA_TEST` | partial | Dynamic alpha test and cutoff correctness. |
| `F_CLOAK` | not fully handled | Cloak/glass approximation or exact shader probe. |
| `F_COSMIC_VEIL` | unverified in hero set | Scan and classify. |
| `F_DETAIL` | mostly missing | Detail texture, blend modes, UV routing, animation. |
| `F_DISABLE_DEPTH_WRITE` | partial | Render-state validation. |
| `F_DISABLE_NPR_OUTLINE` | incomplete | Outline routing. |
| `F_DISABLE_Z_PREPASS` | partial | Render-state validation. |
| `F_ENABLE_TEXTURE_TRANSFORMS` | missing | Shared UV transform path. |
| `F_GLASS` | approximated | Glass mask and cloak interaction. |
| `F_JITTER_VERTICES` | mostly missing | Vertex animation or documented deferral. |
| `F_MATERIAL_VARIANT` | unverified | Scan and classify. |
| `F_NO_SPECULAR_AT_FULL_ROUGHNESS` | missing | Implement or prove irrelevant. |
| `F_OVERRIDE_BLOOM_AMOUNT` | missing | Self-illum/bloom approximation. |
| `F_OVERRIDE_NPR_OUTLINE` | incomplete | Outline thickness override. |
| `F_PAINT_VERTEX_COLORS` | partial | Vertex-color route. |
| `F_RENDER_BACKFACES` | partial | Side/cull-state validation. |
| `F_SECONDARY_UV` | geometry handled, material routing not | UV1 routing per slot. |
| `F_SELF_ILLUM` | wrong | Scale-first glow model (gate on `g_flSelfIllumScale1`, not on flag presence or mask meaningfulness). |
| `F_SHEEN` | partial glTF mapping | Source 2 sheen tint/mask validation. |
| `F_SOLID_COLOR_OUTLINE` | incomplete | Outline tint/additive. |
| `F_TRANSLUCENT` | partial | Blend/depth validation. |
| `F_UNLIT` | partial | Non-NPR unlit/self-illum branch. |
| `F_USE_FRONT_FACE_NORMALS_FOR_BACK_FACES` | missing | Cull/normal approximation. |
| `F_USE_NPR_LIGHTING` | approximated | Complete NPR subsystem routing. |
| `F_USE_STATUS_EFFECTS_PROXY` | not modeled | Classify as engine-runtime effect or implement a fake preview mode. |
| `F_VERTEX_COLOR` | export conditional | Consume in the renderer. |
| `F_WRITE_DEPTH_BEFORE_ALPHA_BLENDING` | partial | Render-state validation. |

### Detail blend modes

`g_nDetailBlendMode` (and the equivalent self-illum/detail blend selectors): `0 = Add`, `1 = Add Self Illum` (requires self-illum), `2 = Mod2X`.

### NPR engine globals

`F_USE_NPR_LIGHTING` materials depend on engine-level attributes that are not present in the per-material VMAT and must be supplied as named defaults: `g_flNPRDiffuseStepSharpness`, `g_flNPRDiffusePbrBlend`, `g_flNPRDirectLightWrap`, `g_flNPRExposureControlPbrBlend`, `g_flNPRSpecularReflectance`, `g_flNPRSpecularStepSharpness`, `g_flNPRSpecularTint`, `g_nNPRSpecularSteps`, `g_vNPRLightWeights`, `g_vNPRExposureTargets`, plus the rim-light controls (`g_flNPRRimLightStrength`, `g_flNPRRimLightFalloff`, `g_flNPRRimLightWrap`, `g_vNPRRimLightUpRamp`) and the boolean stage toggles (`g_bNPRDirectDiffuse`, `g_bNPRBounceDiffuse`, `g_bNPRDirectSpecular`, `g_bNPRExposureControl`, `g_bNPRRimLighting`, `g_bNPRRimLightingDepthOcclusion`).
