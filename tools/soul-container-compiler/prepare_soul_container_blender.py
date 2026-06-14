#!/usr/bin/env python3
"""Prepare GLB soul-container content for Source 2 resourcecompiler.

Run by Blender, not regular Python:

    blender --background --python prepare_soul_container_blender.py -- input.glb out_dir

Environment:
    S2_MODEL_REL                 Source-relative model folder.
    S2_TARGET_LARGEST_AXIS       Largest compiled Source-unit axis target.
    S2_SOURCE_UNITS_PER_BLENDER  Empirical FBX->Source unit multiplier.
"""

from __future__ import annotations

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
    raise SystemExit("usage: prepare_soul_container_blender.py -- <input.glb> <out_dir>")

INPUT_GLB = Path(ARGS[0]).resolve()
OUT_DIR = Path(ARGS[1]).resolve()
MODEL_REL = os.environ.get("S2_MODEL_REL", "models/props_gameplay/soul_container")
MAT_REL = f"{MODEL_REL}/materials"
TARGET_LARGEST_AXIS = float(os.environ.get("S2_TARGET_LARGEST_AXIS", "12.65"))
SOURCE_UNITS_PER_BLENDER = float(os.environ.get("S2_SOURCE_UNITS_PER_BLENDER", "100"))
PHYSICS_RADIUS = float(os.environ.get("S2_PHYSICS_RADIUS", "7.0"))


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
    return [obj for obj in bpy.context.scene.objects if obj.type == "MESH" and obj.data]


def normalize_meshes(meshes) -> None:
    if not meshes:
        raise RuntimeError(f"no mesh objects imported from {INPUT_GLB}")

    for obj in meshes:
        if obj.data.users > 1:
            obj.data = obj.data.copy()

    points = []
    for obj in meshes:
        world = obj.matrix_world.copy()
        points.extend(world @ Vector(corner) for corner in obj.bound_box)

    bounds_min = Vector((min(p.x for p in points), min(p.y for p in points), min(p.z for p in points)))
    bounds_max = Vector((max(p.x for p in points), max(p.y for p in points), max(p.z for p in points)))
    center = (bounds_min + bounds_max) * 0.5
    span = bounds_max - bounds_min
    largest_axis = max(span.x, span.y, span.z)
    if largest_axis <= 0:
        raise RuntimeError(f"invalid imported model bounds: min={tuple(bounds_min)} max={tuple(bounds_max)}")

    scale = TARGET_LARGEST_AXIS / (largest_axis * SOURCE_UNITS_PER_BLENDER)
    normalizer = Matrix.Diagonal((scale, scale, scale, 1.0)) @ Matrix.Translation(-center)

    for obj in meshes:
        world = obj.matrix_world.copy()
        obj.parent = None
        obj.matrix_world = Matrix.Identity(4)
        obj.data.transform(normalizer @ world)
        obj.data.update()

    print(
        "normalized GLB bounds "
        f"min={tuple(bounds_min)} max={tuple(bounds_max)} span={tuple(span)} "
        f"scale={scale} target_source_axis={TARGET_LARGEST_AXIS}"
    )


def used_materials(meshes):
    materials = []
    seen = set()
    for obj in meshes:
        for slot in obj.material_slots:
            mat = slot.material
            if mat and mat.name not in seen:
                materials.append(mat)
                seen.add(mat.name)

    if materials:
        return materials

    mat = bpy.data.materials.new("default")
    mat.use_nodes = True
    for obj in meshes:
        obj.data.materials.append(mat)
    return [mat]


def write_vmat(mat, idx: int, used_names: set[str]) -> None:
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

    vmat_abs.write_text(
        '"Layer0"\n'
        "{\n"
        '    "shader" "pbr.vfx"\n\n'
        '    "F_SELF_ILLUM" "1"\n'
        '    "F_USE_NPR_LIGHTING" "1"\n'
        '    "F_USE_STATUS_EFFECTS_PROXY" "1"\n\n'
        f'    "TextureColor" "{texture_rel}"\n'
        f'    "TextureColor1" "{texture_rel}"\n\n'
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
    (OUT_DIR / "soul_container.vmdl").write_text(
        "<!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} "
        "format:modeldoc28:version{fb63b6ca-f435-4aa0-a2c7-c66ddc651dca} -->\n"
        "{\n"
        "\trootNode = \n"
        "\t{\n"
        '\t\t_class = "RootNode"\n'
        "\t\tchildren = \n"
        "\t\t[\n"
        "\t\t\t{\n"
        '\t\t\t\t_class = "BoneMarkupList"\n'
        "\t\t\t\tchildren = [ ]\n"
        '\t\t\t\tbone_cull_type = "None"\n'
        "\t\t\t},\n"
        "\t\t\t{\n"
        '\t\t\t\t_class = "RenderMeshList"\n'
        "\t\t\t\tchildren = \n"
        "\t\t\t\t[\n"
        "\t\t\t\t\t{\n"
        '\t\t\t\t\t\t_class = "RenderMeshFile"\n'
        '\t\t\t\t\t\tname = "soul_container"\n'
        f'\t\t\t\t\t\tfilename = "{MODEL_REL}/model.fbx"\n'
        "\t\t\t\t\t},\n"
        "\t\t\t\t]\n"
        "\t\t\t},\n"
        "\t\t\t{\n"
        '\t\t\t\t_class = "Skeleton"\n'
        "\t\t\t\tchildren = \n"
        "\t\t\t\t[\n"
        "\t\t\t\t\t{\n"
        '\t\t\t\t\t\t_class = "Bone"\n'
        '\t\t\t\t\t\tname = "joint1"\n'
        "\t\t\t\t\t\torigin = [ 0.0, 0.0, 0.0 ]\n"
        "\t\t\t\t\t\tangles = [ 0.0, 90.0, 90.0 ]\n"
        "\t\t\t\t\t\tdo_not_discard = true\n"
        "\t\t\t\t\t},\n"
        "\t\t\t\t]\n"
        "\t\t\t},\n"
        "\t\t\t{\n"
        '\t\t\t\t_class = "PhysicsShapeList"\n'
        "\t\t\t\tchildren = \n"
        "\t\t\t\t[\n"
        "\t\t\t\t\t{\n"
        '\t\t\t\t\t\t_class = "PhysicsShapeSphere"\n'
        '\t\t\t\t\t\tparent_bone = "joint1"\n'
        '\t\t\t\t\t\tsurface_prop = "hideout_ball"\n'
        '\t\t\t\t\t\tcollision_tags = ""\n'
        f"\t\t\t\t\t\tradius = {PHYSICS_RADIUS}\n"
        "\t\t\t\t\t\tcenter = [ 0.0, 0.0, 0.0 ]\n"
        '\t\t\t\t\t\tname = ""\n'
        "\t\t\t\t\t},\n"
        "\t\t\t\t]\n"
        "\t\t\t},\n"
        "\t\t]\n"
        "\t}\n"
        "}\n",
        encoding="utf-8",
        newline="\n",
    )


def main() -> None:
    if not INPUT_GLB.is_file():
        raise RuntimeError(f"input GLB not found: {INPUT_GLB}")

    (OUT_DIR / "materials").mkdir(parents=True, exist_ok=True)

    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete()
    bpy.ops.import_scene.gltf(filepath=str(INPUT_GLB))

    meshes = mesh_objects()
    normalize_meshes(meshes)

    used_names: set[str] = set()
    for idx, mat in enumerate(used_materials(meshes)):
        write_vmat(mat, idx, used_names)

    bpy.ops.object.select_all(action="DESELECT")
    for obj in meshes:
        obj.select_set(True)
    bpy.context.view_layer.objects.active = meshes[0]

    bpy.ops.export_scene.fbx(
        filepath=str(OUT_DIR / "model.fbx"),
        use_selection=True,
        bake_anim=False,
        add_leaf_bones=False,
        path_mode="RELATIVE",
    )

    write_vmdl()
    print(f"wrote Source 2 soul-container content to {OUT_DIR}")


main()
