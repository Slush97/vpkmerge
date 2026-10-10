#!/usr/bin/env python3
"""Emit a modeldoc28 .vmdl whose Skeleton node carries the full donor skeleton
with correct Source-space parent-relative transforms, so resourcecompiler keeps
every bone (not just the skin-weighted ones) and the donor hero's animgraph can
drive the swapped mesh.

Input: skeleton JSON from `dump_skeleton` (name, parent index, pos[3] = Source
parent-relative translation, quat[4] = Source parent-relative rotation).
Quaternions are converted to Source QAngle [pitch, yaw, roll] via the engine's
QuaternionMatrix + MatrixAngles.

usage: emit_skeleton_vmdl.py <skeleton.json> <out.vmdl> <model_rel> <model_name>
"""
import json
import math
import os
import sys


def quat_to_qangle(x, y, z, w):
    # Source QuaternionMatrix: columns are forward/left/up (m[row][col]).
    m00 = 1 - 2 * y * y - 2 * z * z
    m10 = 2 * x * y + 2 * w * z
    m20 = 2 * x * z - 2 * w * y
    m01 = 2 * x * y - 2 * w * z
    m11 = 1 - 2 * x * x - 2 * z * z
    m21 = 2 * y * z + 2 * w * x
    m22 = 1 - 2 * x * x - 2 * y * y
    fwd0, fwd1, fwd2 = m00, m10, m20
    left0, left1, left2 = m01, m11, m21
    up2 = m22
    xy = math.hypot(fwd0, fwd1)
    if xy > 1e-3:
        yaw = math.degrees(math.atan2(fwd1, fwd0))
        pitch = math.degrees(math.atan2(-fwd2, xy))
        roll = math.degrees(math.atan2(left2, up2))
    else:
        yaw = math.degrees(math.atan2(-left0, left1))
        pitch = math.degrees(math.atan2(-fwd2, xy))
        roll = 0.0
    return pitch, yaw, roll


def main():
    skel_json, out_vmdl, model_rel, model_name = sys.argv[1:5]
    # optional 5th arg: animgraph resource to drive the model (RootNode
    # anim_graph_name), e.g. "animgraphs/animgraph2/hero/hero.vnmgraph+familiar.vnmgraph"
    anim_graph = sys.argv[5] if len(sys.argv) > 5 else ""
    bones = json.load(open(skel_json))
    names = [b["name"] for b in bones]
    kids = {}
    for i, b in enumerate(bones):
        kids.setdefault(b["parent"], []).append(i)

    def fmt(v):
        return repr(float(v))

    def emit(idx, depth):
        b = bones[idx]
        p = b["pos"]
        pa, ya, ra = quat_to_qangle(*b["quat"])
        pad = "\t" * depth
        s = pad + "{\n"
        s += pad + '\t_class = "Bone"\n'
        s += pad + f'\tname = "{b["name"]}"\n'
        s += pad + f"\torigin = [ {fmt(p[0])}, {fmt(p[1])}, {fmt(p[2])} ]\n"
        s += pad + f"\tangles = [ {fmt(pa)}, {fmt(ya)}, {fmt(ra)} ]\n"
        s += pad + "\tdo_not_discard = true\n"
        ch = kids.get(idx, [])
        if ch:
            s += pad + "\tchildren =\n" + pad + "\t[\n"
            s += "".join(emit(c, depth + 2) for c in ch)
            s += pad + "\t]\n"
        s += pad + "},\n"
        return s

    # optional 6th/7th args: hero gamedata prefab to merge + NM skeleton ref
    prefab = os.environ.get("S2_PREFAB", sys.argv[6] if len(sys.argv) > 6 else "").strip()
    nmskel = os.environ.get("S2_NMSKEL", sys.argv[7] if len(sys.argv) > 7 else "").strip()

    roots = kids.get(-1, [])
    skel = "".join(emit(r, 5) for r in roots)
    fbx_rel = f"{model_rel}/model.fbx"
    # Hero animgraph is driven through a CCitadelHeroModelGameData_t GameData
    # node (NOT the RootNode anim_graph_name, which heroes leave empty). This is
    # what makes the NM motion-matching graph actually tick in-game. Pattern from
    # CSDK hero_uipose_template/.../bebop_heromodelgamedata.vmdl_prefab.
    # The AttachmentCameraData GenericGameData (camera-FOV node) is merged into the
    # SAME GameDataList as CCitadelHeroModelGameData_t (vanilla heroes use a single
    # GameDataList; a second one is dropped by resourcecompiler).
    cameradata_gamedata = ""
    cameradata_file = os.environ.get("S2_CAMERADATA_FILE", "").strip()
    if cameradata_file and os.path.isfile(cameradata_file):
        raw = open(cameradata_file).read().strip()
        if not raw.endswith(","):
            raw += ","
        cameradata_gamedata = raw + "\n"
    anim_line = ""
    gamedata_node = ""
    if anim_graph:
        ui_graph = anim_graph.replace("/hero.vnmgraph", "/hero_ui.vnmgraph")
        gamedata_node = (
            "\t\t\t{\n\t\t\t\t_class = \"GameDataList\"\n\t\t\t\tchildren =\n\t\t\t\t[\n"
            "\t\t\t\t\t{\n\t\t\t\t\t\t_class = \"GenericGameData\"\n"
            "\t\t\t\t\t\tgame_class = \"CCitadelHeroModelGameData_t\"\n"
            "\t\t\t\t\t\tgame_keys =\n\t\t\t\t\t\t{\n"
            f"\t\t\t\t\t\t\tm_sAG2HeroPawnAnimGraph = resource_name:\"{anim_graph}\"\n"
            f"\t\t\t\t\t\t\tm_sAG2UIAnimGraph = resource_name:\"{ui_graph}\"\n"
            "\t\t\t\t\t\t\tm_bUseAG2HeroGraph = true\n"
            "\t\t\t\t\t\t\tm_bUseAG2UIGraph = true\n"
            "\t\t\t\t\t\t}\n\t\t\t\t\t},\n"
            + cameradata_gamedata
            + "\t\t\t\t]\n\t\t\t},\n"
        )
    prefab_node = (
        "\t\t\t{\n\t\t\t\t_class = \"Prefab\"\n"
        f"\t\t\t\ttarget_file = \"{prefab}\"\n\t\t\t}},\n"
    ) if prefab else ""
    nmskel_line = f'\t\tmodel_skeleton_name = "{nmskel}"\n' if nmskel else ""
    # Optional: an AttachmentList node lifted from the donor hero's decompiled
    # .vmdl (oracle `vmdl-source`). Heroes' camera is driven by attachment points
    # (standing_pivot / crouching_pivot / near_00 / far_00 / gunaim_00 / etc.,
    # referenced by the keyValueText AttachmentCameraData), plus the IK/weapon
    # attachments. A bare mesh+skeleton compile has none, so the camera falls back
    # to a default framing. Injecting the donor's AttachmentList verbatim (its
    # parent_bones are all in the injected skeleton) restores the real camera.
    attach_node = ""
    attach_file = os.environ.get("S2_ATTACHMENT_FILE", "").strip()
    if attach_file and os.path.isfile(attach_file):
        raw = open(attach_file).read().strip()
        if not raw.endswith(","):
            raw += ","
        attach_node = raw + "\n"
    vmdl = (
        "<!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} "
        "format:modeldoc36:version{972dada4-b828-45a4-bb93-7795cf0585da} -->\n"
        "{\n\trootNode =\n\t{\n\t\t_class = \"RootNode\"\n"
        + '\t\tmodel_archetype = "citadel_hero"\n'
        + anim_line
        + nmskel_line
        + "\t\tchildren =\n\t\t[\n"
        + gamedata_node
        + prefab_node
        + "\t\t\t{\n\t\t\t\t_class = \"BoneMarkupList\"\n\t\t\t\tchildren = [ ]\n"
        "\t\t\t\tbone_cull_type = \"None\"\n\t\t\t},\n"
        "\t\t\t{\n\t\t\t\t_class = \"RenderMeshList\"\n\t\t\t\tchildren =\n\t\t\t\t[\n"
        "\t\t\t\t\t{\n\t\t\t\t\t\t_class = \"RenderMeshFile\"\n"
        f"\t\t\t\t\t\tname = \"{model_name}\"\n"
        f"\t\t\t\t\t\tfilename = \"{fbx_rel}\"\n"
        "\t\t\t\t\t\timport_scale = 1.0\n\t\t\t\t\t},\n"
        "\t\t\t\t]\n\t\t\t},\n"
        "\t\t\t{\n\t\t\t\t_class = \"Skeleton\"\n\t\t\t\tchildren =\n\t\t\t\t[\n"
        + skel
        + "\t\t\t\t]\n\t\t\t},\n"
        + attach_node
        + "\t\t]\n\t}\n}\n"
    )
    open(out_vmdl, "w").write(vmdl)
    print(f"wrote {out_vmdl}: {len(names)} bones")


if __name__ == "__main__":
    main()
