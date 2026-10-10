#!/usr/bin/env python3
"""Prepare a rigged GLB (mesh + skeleton + skin weights) for Source 2
resourcecompiler as a skinned hero-model swap.

Unlike the soul-container staging script (a static prop: it normalizes a fresh
mesh to a target size and ships a 1-bone skeleton), this keeps the imported
armature and skin intact and only *uniformly scales* the whole rig so the
compiled model lands at the donor hero's real size. The skeleton + skin weights
ride along inside the FBX, so the compiled `.vmdl_c` carries the donor's bones
(by name) and the engine's existing animgraph can drive it.

Run by Blender, not regular Python:

    blender --background --python prepare_hero_blender.py -- input.glb out_dir

Environment:
    S2_MODEL_REL                 Source-relative model folder (e.g. models/heroes_wip/familiar).
    S2_MODEL_NAME                base name of the .vmdl / RenderMeshFile (e.g. familiar).
    S2_TARGET_SOURCE_HEIGHT      target compiled height in Source units (donor hero height).
    S2_SOURCE_UNITS_PER_BLENDER  empirical FBX->Source unit multiplier (default 100).
"""

from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path

import bpy
from mathutils import Matrix, Vector


def script_args() -> list[str]:
    if "--" in sys.argv:
        return sys.argv[sys.argv.index("--") + 1 :]
    return sys.argv[1:]


ARGS = script_args()
if len(ARGS) != 2:
    raise SystemExit("usage: prepare_hero_blender.py -- <input.glb> <out_dir>")

INPUT_GLB = Path(ARGS[0]).resolve()
OUT_DIR = Path(ARGS[1]).resolve()
MODEL_REL = os.environ.get("S2_MODEL_REL", "models/heroes_wip/familiar")
MODEL_NAME = os.environ.get("S2_MODEL_NAME", "familiar")
MAT_REL = f"{MODEL_REL}/materials"
TARGET_SOURCE_HEIGHT = float(os.environ.get("S2_TARGET_SOURCE_HEIGHT", "109.0"))
SOURCE_UNITS_PER_BLENDER = float(os.environ.get("S2_SOURCE_UNITS_PER_BLENDER", "100"))
# Optional map {safe_material_basename: "source/rel/path/to/existing.vmat (no ext)"}.
# A material listed here is NOT regenerated; the FBX slot is renamed to the given
# pak-relative path so the compiled model references an EXISTING shipped material
# (e.g. point a swapped mesh's slot at Valve's real ghost material instead of a
# generated stand-in). Keeps the authentic shader/flags/textures with zero authoring.
MATERIAL_REMAP = json.loads(os.environ.get("S2_MATERIAL_REMAP", "{}"))
# Optional map {safe_material_basename: "path/to/template.vmat"}. A material listed
# here still bakes its albedo PNG, but its .vmat is written from the template file
# (with "{{TEXTURE}}" replaced by the baked albedo's pak-relative path) instead of the
# generic pbr.vfx recipe. Lets a swap carry a custom shader recipe per material (e.g. an
# unlit-shadow body + a self-illum glow) without hand-editing this script.
VMAT_TEMPLATE = json.loads(os.environ.get("S2_VMAT_TEMPLATE", "{}"))
# Yaw (degrees about world Z) applied to the whole rig before scale/export, so an
# operator can correct facing without hand-editing this script. Source forward is
# +X; a model that imports facing the wrong way needs e.g. 90 / -90 / 180 here.
# Default 0 = no rotation (keeps the proven hat-man path byte-for-byte unchanged).
FACING_YAW_DEG = float(os.environ.get("S2_FACING_YAW_DEG", "0"))
# Optional rebind: path to a dump_skeleton JSON (Source parent-relative
# pos/quat). Bones present in both the JSON and the armature are POSED to the
# JSON's world binds, the deformation is baked into every mesh, and the pose is
# applied as the new rest. Use when the input rig's rest pose drifts from the
# runtime skeleton the engine animates (the vnmskel): animated bones anchor at
# THEIR positions in-game, so any rest-pose drift displaces the covered
# geometry (Baroness Mina's hair sat ~1 unit off the scalp). Bones absent from
# the JSON (custom additions) ride their corrected parents.
REBIND_JSON = os.environ.get("S2_REBIND_JSON", "").strip()


def safe_name(name: str, fallback: str) -> str:
    value = re.sub(r"[^a-zA-Z0-9_]+", "_", name.strip().lower()).strip("_")
    return value or fallback


def linked_image_from_socket(socket, seen=None):
    if seen is None:
        seen = set()
    for link in socket.links:
        node = link.from_node
        if node in seen:
            continue
        seen.add(node)
        if node.bl_idname == "ShaderNodeTexImage" and node.image:
            return node.image
        for input_socket in getattr(node, "inputs", []):
            if input_socket.is_linked:
                found = linked_image_from_socket(input_socket, seen)
                if found:
                    return found
    return None


def material_base_color(mat):
    color = (1.0, 1.0, 1.0, 1.0)
    if not mat or not mat.use_nodes:
        return color
    for node in mat.node_tree.nodes:
        if node.bl_idname == "ShaderNodeBsdfPrincipled":
            base = node.inputs.get("Base Color")
            if base:
                return tuple(base.default_value)
    return color


def material_image(mat):
    if not mat or not mat.use_nodes:
        return None
    for node in mat.node_tree.nodes:
        if node.bl_idname == "ShaderNodeBsdfPrincipled":
            base = node.inputs.get("Base Color")
            if base and base.is_linked:
                found = linked_image_from_socket(base)
                if found:
                    return found
    for node in mat.node_tree.nodes:
        if node.bl_idname == "ShaderNodeTexImage" and node.image:
            return node.image
    return None


def save_image_or_color(mat, out_path: Path) -> None:
    img = material_image(mat)
    if img:
        img.filepath_raw = str(out_path)
        img.file_format = "PNG"
        img.save()
        return
    rgba = material_base_color(mat)
    generated = bpy.data.images.new(out_path.name, width=2, height=2, alpha=True)
    generated.pixels = list(rgba) * 4
    generated.filepath_raw = str(out_path)
    generated.file_format = "PNG"
    generated.save()


def mesh_objects():
    return [o for o in bpy.context.scene.objects if o.type == "MESH" and o.data]


def armature_object():
    for o in bpy.context.scene.objects:
        if o.type == "ARMATURE":
            return o
    return None


def rotate_rig(arm, meshes, yaw_deg: float) -> None:
    """Rotate the whole rig about world Z (pivot at origin) so it faces Source
    forward (+X), then bake the rotation in. No-op when yaw_deg == 0."""
    if abs(yaw_deg) < 1e-6:
        return
    import math

    bpy.ops.object.select_all(action="DESELECT")
    objs = ([arm] + meshes) if arm else meshes
    for o in objs:
        o.select_set(True)
    bpy.context.view_layer.objects.active = arm or meshes[0]
    bpy.context.scene.cursor.location = (0.0, 0.0, 0.0)
    prev = bpy.context.scene.tool_settings.transform_pivot_point
    bpy.context.scene.tool_settings.transform_pivot_point = "CURSOR"
    bpy.ops.transform.rotate(value=math.radians(yaw_deg), orient_axis="Z")
    bpy.context.scene.tool_settings.transform_pivot_point = prev
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=False)
    print(f"rotated rig {yaw_deg:.1f} deg about Z to face Source +X")


def scale_rig(arm, meshes) -> float:
    """Uniformly scale the whole rig about the world origin so the compiled
    model hits TARGET_SOURCE_HEIGHT given the empirical FBX multiplier."""
    pts = []
    for o in meshes:
        for c in o.bound_box:
            pts.append(o.matrix_world @ Vector(c))
    zmin = min(p.z for p in pts)
    zmax = max(p.z for p in pts)
    cur_h = zmax - zmin
    if cur_h <= 0:
        raise RuntimeError("invalid rig height")
    target_blender_h = TARGET_SOURCE_HEIGHT / SOURCE_UNITS_PER_BLENDER
    s = target_blender_h / cur_h

    bpy.ops.object.select_all(action="DESELECT")
    objs = [arm] + meshes if arm else meshes
    for o in objs:
        o.select_set(True)
    bpy.context.view_layer.objects.active = arm or meshes[0]
    # pivot at world origin
    bpy.context.scene.cursor.location = (0.0, 0.0, 0.0)
    prev = bpy.context.scene.tool_settings.transform_pivot_point
    bpy.context.scene.tool_settings.transform_pivot_point = "CURSOR"
    bpy.ops.transform.resize(value=(s, s, s))
    bpy.context.scene.tool_settings.transform_pivot_point = prev
    # bake transforms so the FBX is clean (armature scale -> 1.0)
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    print(f"scaled rig by {s:.5f}: blender height {cur_h:.4f} -> {cur_h*s:.4f} "
          f"(target source {TARGET_SOURCE_HEIGHT} @ {SOURCE_UNITS_PER_BLENDER}/blender)")
    return s


def used_materials(meshes):
    materials, seen = [], set()
    for o in meshes:
        for slot in o.material_slots:
            m = slot.material
            if m and m.name not in seen:
                materials.append(m)
                seen.add(m.name)
    if materials:
        return materials
    m = bpy.data.materials.new("default")
    m.use_nodes = True
    for o in meshes:
        o.data.materials.append(m)
    return [m]


def write_vmat(mat, idx, used_names) -> None:
    base = safe_name(mat.name, f"material_{idx:02d}")
    name = base
    suffix = 1
    while name in used_names:
        suffix += 1
        name = f"{base}_{suffix}"
    used_names.add(name)
    texture_rel = f"{MAT_REL}/{name}_color.png"
    texture_abs = OUT_DIR / "materials" / f"{name}_color.png"
    vmat_abs = OUT_DIR / "materials" / f"{name}.vmat"
    save_image_or_color(mat, texture_abs)
    if base in VMAT_TEMPLATE:
        tmpl = Path(VMAT_TEMPLATE[base]).read_text(encoding="utf-8")
        vmat_abs.write_text(
            tmpl.replace("{{TEXTURE}}", texture_rel),
            encoding="utf-8", newline="\n")
        print(f"vmat from template: {base} <- {VMAT_TEMPLATE[base]}")
        mat.name = f"{MAT_REL}/{name}"
        return
    # Proven soul-container recipe (renders albedo textures in-game). The
    # simplified TextureColor-only form left the hero swap flat white.
    #
    # The three extra feature flags are the ones the ModelDoc material-editor
    # workflow says to "always click" on a hero material, and which the generic
    # recipe was missing:
    #   F_SOLID_COLOR_OUTLINE         -> the signature Deadlock black silhouette
    #                                    outline (151/605 shipped hero materials
    #                                    set it; without it the swap reads flatter
    #                                    than a stock hero).
    #   F_WRITE_DEPTH_BEFORE_ALPHA_BLENDING -> opaque depth prepass so the hero
    #                                    sorts correctly against its own alpha and
    #                                    the world (avoids see-through artefacts).
    # TextureRoughness1 is the constant fallback for the unbound roughness
    # sampler; white = fully matte, which kills the default plastic shine the
    # bare recipe leaves on (the tutorial drags the roughness slider to white).
    vmat_abs.write_text(
        '"Layer0"\n{\n'
        '    "shader" "pbr.vfx"\n\n'
        '    "F_USE_NPR_LIGHTING" "1"\n'
        '    "F_USE_STATUS_EFFECTS_PROXY" "1"\n'
        '    "F_SOLID_COLOR_OUTLINE" "1"\n'
        '    "F_WRITE_DEPTH_BEFORE_ALPHA_BLENDING" "1"\n\n'
        f'    "TextureColor" "{texture_rel}"\n'
        f'    "TextureColor1" "{texture_rel}"\n\n'
        '    "TextureRoughness1" "[1.000 1.000 1.000 0.000]"\n\n'
        '    "g_bMaskColorTint1" "1"\n'
        '    "g_bMaskVertexColorTint1" "1"\n'
        '    "g_nTextureColorTintMode1" "0"\n'
        '    "g_vColorTint1" "[1 1 1 0]"\n'
        '    "g_fVertexColorStrength1" "1"\n\n'
        '    "g_flSelfIllumAlbedoFactor1" "1"\n'
        '    "g_flSelfIllumScale1" "0"\n'
        "}\n",
        encoding="utf-8",
        newline="\n",
    )
    mat.name = f"{MAT_REL}/{name}"


def write_vmdl() -> None:
    fbx_rel = f"{MODEL_REL}/model.fbx"
    # Optional: emit the CCitadelHeroModelGameData_t gamedata node that marks the
    # model an AG2 hero (m_bUseAG2HeroGraph) and names its pawn/UI animgraphs.
    # This is the half CSDK 12 *can* compile; the matching precomputed
    # m_animGraph2Refs / m_vecNmSkeletonRefs fields (whose ModelDoc node classes
    # CSDK lacks) are injected post-compile with morphic. Env-gated:
    #   S2_ANIMGRAPH    = animgraphs/animgraph2/hero/hero.vnmgraph+<codename>.vnmgraph
    #   S2_UI_ANIMGRAPH = animgraphs/animgraph2/hero/hero_ui.vnmgraph+<codename>.vnmgraph
    animgraph = os.environ.get("S2_ANIMGRAPH", "").strip()
    ui_animgraph = os.environ.get("S2_UI_ANIMGRAPH", "").strip()
    extra = ""
    if animgraph:
        extra += (
            "\t\t\t{\n"
            '\t\t\t\t_class = "GameDataList"\n'
            "\t\t\t\tchildren =\n\t\t\t\t[\n"
            "\t\t\t\t\t{\n"
            '\t\t\t\t\t\t_class = "GenericGameData"\n'
            '\t\t\t\t\t\tname = ""\n'
            '\t\t\t\t\t\tgame_class = "CCitadelHeroModelGameData_t"\n'
            "\t\t\t\t\t\tgame_keys =\n\t\t\t\t\t\t{\n"
            f'\t\t\t\t\t\t\tm_sAG2HeroPawnAnimGraph = resource_name:"{animgraph}"\n'
            "\t\t\t\t\t\t\tm_bUseAG2HeroGraph = true\n"
            f'\t\t\t\t\t\t\tm_sAG2UIAnimGraph = resource_name:"{ui_animgraph}"\n'
            "\t\t\t\t\t\t\tm_bUseAG2UIGraph = true\n"
            "\t\t\t\t\t\t}\n"
            "\t\t\t\t\t},\n"
            "\t\t\t\t]\n"
            "\t\t\t},\n"
        )
    (OUT_DIR / f"{MODEL_NAME}.vmdl").write_text(
        "<!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} "
        "format:modeldoc28:version{fb63b6ca-f435-4aa0-a2c7-c66ddc651dca} -->\n"
        "{\n"
        "\trootNode =\n\t{\n"
        '\t\t_class = "RootNode"\n'
        "\t\tchildren =\n\t\t[\n"
        "\t\t\t{\n"
        '\t\t\t\t_class = "BoneMarkupList"\n'
        "\t\t\t\tchildren = [ ]\n"
        '\t\t\t\tbone_cull_type = "None"\n'
        "\t\t\t},\n"
        "\t\t\t{\n"
        '\t\t\t\t_class = "RenderMeshList"\n'
        "\t\t\t\tchildren =\n\t\t\t\t[\n"
        "\t\t\t\t\t{\n"
        '\t\t\t\t\t\t_class = "RenderMeshFile"\n'
        f'\t\t\t\t\t\tname = "{MODEL_NAME}"\n'
        f'\t\t\t\t\t\tfilename = "{fbx_rel}"\n'
        "\t\t\t\t\t\timport_scale = 1.0\n"
        "\t\t\t\t\t},\n"
        "\t\t\t\t]\n"
        "\t\t\t},\n"
        f"{extra}"
        "\t\t]\n"
        "\t}\n"
        "}\n",
        encoding="utf-8",
        newline="\n",
    )


def skel_world_transforms(skel):
    """dump_skeleton JSON -> {bone: 4x4 world Matrix} (Source space)."""
    from mathutils import Matrix, Quaternion, Vector

    worlds = {}

    def compute(i):
        b = skel[i]
        if b["name"] in worlds:
            return worlds[b["name"]]
        x, y, z, w = b["quat"]
        m = Quaternion((w, x, y, z)).to_matrix().to_4x4()
        m.translation = Vector(b["pos"])
        if b["parent"] >= 0:
            m = compute(b["parent"]) @ m
        worlds[b["name"]] = m
        return m

    for i in range(len(skel)):
        compute(i)
    return worlds


def rebind_rig(arm, meshes, rebind_json: str) -> None:
    """Reposition named REST bones to the JSON's world binds; geometry untouched.

    Use when the input rig's rest pose drifted from the runtime skeleton
    through a decompile/Blender round-trip while the mesh itself was authored
    for (and still matches) the runtime binds: the verts are correct, only the
    bones moved. Repositioning rest bones in edit mode re-pairs the unchanged
    geometry with the runtime binds, exactly reproducing the original
    compiled pairing. Bones absent from the JSON (custom additions) are
    translated by their nearest repositioned ancestor's delta so they stay
    glued to the geometry they carry.
    """
    import json as _json
    from mathutils import Vector

    _ = meshes  # geometry is deliberately untouched
    skel = _json.loads(Path(rebind_json).read_text())
    targets = skel_world_transforms(skel)
    arm_inv = arm.matrix_world.inverted()

    bpy.ops.object.select_all(action="DESELECT")
    arm.select_set(True)
    bpy.context.view_layer.objects.active = arm
    bpy.ops.object.mode_set(mode="EDIT")
    ebs = arm.data.edit_bones
    old_heads = {eb.name: eb.head.copy() for eb in ebs}
    moved = 0
    for eb in ebs:
        if eb.name not in targets:
            continue
        eb.use_connect = False
        length = eb.length
        eb.matrix = arm_inv @ targets[eb.name]
        eb.length = length
        moved += 1
    # Untargeted bones (custom additions) follow their nearest repositioned
    # ancestor's translation so they keep riding the same body part.
    adjusted = 0
    for eb in ebs:
        if eb.name in targets:
            continue
        anc = eb.parent
        while anc is not None and anc.name not in targets:
            anc = anc.parent
        if anc is None:
            continue
        delta = anc.head - old_heads[anc.name]
        if delta.length > 1e-9:
            eb.use_connect = False
            eb.translate(Vector(delta))
            adjusted += 1
    bpy.ops.object.mode_set(mode="OBJECT")
    print(
        f"rebind: repositioned {moved}/{len(arm.data.bones)} rest bones to "
        f"{rebind_json} (geometry untouched, {adjusted} custom bones followed parents)"
    )


def dump_rig_skeleton(arm, out_path: Path) -> None:
    """Write the armature's REST pose as a dump_skeleton-shaped JSON in Source
    space (pre-scale). This is the skeleton the FBX carries, so emitting the
    .vmdl Skeleton node from it keeps engine binds and RC mesh binds identical
    by construction."""
    import json as _json

    bones = list(arm.data.bones)
    index = {b.name: i for i, b in enumerate(bones)}
    out = []
    for b in bones:
        if b.parent is not None:
            rel = b.parent.matrix_local.inverted() @ b.matrix_local
            parent = index[b.parent.name]
        else:
            rel = arm.matrix_world @ b.matrix_local
            parent = -1
        q = rel.to_quaternion()  # (w, x, y, z)
        out.append(
            {
                "name": b.name,
                "parent": parent,
                "pos": list(rel.translation),
                "quat": [q.x, q.y, q.z, q.w],
            }
        )
    out_path.write_text(_json.dumps(out, indent=1))
    print(f"wrote rig skeleton ({len(out)} bones) to {out_path}")


def main() -> None:
    if not INPUT_GLB.is_file():
        raise RuntimeError(f"input GLB not found: {INPUT_GLB}")
    (OUT_DIR / "materials").mkdir(parents=True, exist_ok=True)

    if INPUT_GLB.suffix.lower() == ".blend":
        # Open the verified rig directly (a GLB round-trip scrambles the
        # vertex-group -> bone binding, so the .blend is the source of truth).
        bpy.ops.wm.open_mainfile(filepath=str(INPUT_GLB))
    else:
        bpy.ops.object.select_all(action="SELECT")
        bpy.ops.object.delete()
        bpy.ops.import_scene.gltf(filepath=str(INPUT_GLB))

    for o in bpy.data.objects:
        o.hide_set(False)
        o.hide_viewport = False

    # Prune anything that isn't a deforming mesh or its armature: the donor
    # hero's own body, loose export artifacts, and the weightless "Icosphere"
    # bound-sphere proxy VRF/glTF emit. A real skinned swap mesh carries vertex
    # groups; a 0-group mesh is junk. (Generic; replaces the old BMO-only keep.)
    for o in list(bpy.data.objects):
        if o.type == "MESH" and len(o.vertex_groups) == 0:
            print(f"pruning weightless mesh: {o.name}")
            bpy.data.objects.remove(o, do_unlink=True)

    meshes = mesh_objects()
    if not meshes:
        raise RuntimeError("no meshes imported")
    arm = armature_object()
    if not arm:
        raise RuntimeError("no armature imported - need the rigged GLB/blend")

    if REBIND_JSON:
        rebind_rig(arm, meshes, REBIND_JSON)
    rotate_rig(arm, meshes, FACING_YAW_DEG)
    # Dump after rebind/rotate (rest pose final) but BEFORE scale (Source units).
    dump_rig_skeleton(arm, OUT_DIR / "_rig_skeleton.json")
    scale_rig(arm, meshes)

    used_names: set[str] = set()
    for idx, mat in enumerate(used_materials(meshes)):
        base = safe_name(mat.name, f"material_{idx:02d}")
        if base in MATERIAL_REMAP:
            # Reference an existing shipped material by full pak path; don't
            # generate a stand-in. RC bakes this path as the mesh's material ref.
            mat.name = MATERIAL_REMAP[base]
            print(f"remapped material {base} -> {mat.name}")
            continue
        write_vmat(mat, idx, used_names)

    # Select armature + meshes for a skinned FBX (all bones, no anim)
    bpy.ops.object.select_all(action="DESELECT")
    arm.select_set(True)
    for o in meshes:
        o.select_set(True)
    bpy.context.view_layer.objects.active = arm

    bpy.ops.export_scene.fbx(
        filepath=str(OUT_DIR / "model.fbx"),
        use_selection=True,
        object_types={"ARMATURE", "MESH"},
        use_armature_deform_only=False,
        add_leaf_bones=False,
        bake_anim=False,
        mesh_smooth_type="FACE",
        path_mode="STRIP",
        apply_scale_options="FBX_SCALE_NONE",
        global_scale=1.0,
    )

    write_vmdl()
    print(f"wrote skinned hero content to {OUT_DIR}")


main()
