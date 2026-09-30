#!/usr/bin/env python3
"""Merge two dump_skeleton JSONs into one bone list matching a reference GLB rig.

Built for combined-mod imports (Baroness Mina): a Blender GLB unions the meshes
of two compiled mods that each override the same hero model, so its rig is the
union of the two mods' skeletons. Both mods are compiled models, so every bone's
parent-relative transform can be taken verbatim from a dump_skeleton dump; no
glTF coordinate math is needed. The GLB defines the bone SET (and validates the
hierarchy); the JSONs supply order and transforms.

Rules:
- Every GLB joint must exist in the base or extra JSON (error otherwise).
- Output order: base bones (all must be GLB joints) first, then the GLB-only
  extras in the extra JSON's order. Parents are reindexed by name.
- Validation gate: for every output bone whose GLB parent is also a joint, the
  JSON parent must match the GLB hierarchy; a mismatch aborts the merge.

Usage: merge_skeletons.py <base_skel.json> <extra_skel.json> <rig.glb> [-o out.json]
"""

from __future__ import annotations

import argparse
import json
import struct
import sys
from pathlib import Path


def load_glb_joints(path: Path) -> tuple[list[str], dict[str, str]]:
    """Return (joint names, joint -> parent-joint name map) from the GLB's skin."""
    data = path.read_bytes()
    magic, _ver, _length = struct.unpack("<III", data[:12])
    if magic != 0x46546C67:
        sys.exit(f"{path}: not a GLB")
    json_len, chunk_type = struct.unpack("<II", data[12:20])
    if chunk_type != 0x4E4F534A:
        sys.exit(f"{path}: first chunk is not JSON")
    gltf = json.loads(data[20 : 20 + json_len])
    skins = gltf.get("skins") or []
    if not skins:
        sys.exit(f"{path}: no skin")
    joints = skins[0]["joints"]
    nodes = gltf["nodes"]
    names = [nodes[i].get("name", f"node{i}") for i in joints]
    joint_set = set(joints)
    parent_of: dict[str, str] = {}
    for i, node in enumerate(nodes):
        for child in node.get("children", []):
            if child in joint_set and i in joint_set:
                parent_of[nodes[child].get("name", f"node{child}")] = node.get(
                    "name", f"node{i}"
                )
    return names, parent_of


def by_name(skel: list[dict]) -> dict[str, dict]:
    out = {}
    for i, bone in enumerate(skel):
        parent = skel[bone["parent"]]["name"] if bone["parent"] >= 0 else None
        out[bone["name"]] = {**bone, "parent_name": parent, "index": i}
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("base_json", type=Path)
    ap.add_argument("extra_json", type=Path)
    ap.add_argument("glb", type=Path)
    ap.add_argument("-o", "--out", type=Path, default=None)
    args = ap.parse_args()

    base = json.loads(args.base_json.read_text())
    extra = json.loads(args.extra_json.read_text())
    glb_names, glb_parent = load_glb_joints(args.glb)
    glb_set = set(glb_names)

    base_by = by_name(base)
    extra_by = by_name(extra)

    missing = [n for n in glb_names if n not in base_by and n not in extra_by]
    if missing:
        sys.exit(f"ERROR: {len(missing)} GLB joints in neither JSON: {missing}")

    not_in_glb = [b["name"] for b in base if b["name"] not in glb_set]
    if not_in_glb:
        sys.exit(
            f"ERROR: {len(not_in_glb)} base bones absent from the GLB rig "
            f"(base must be the subset skeleton): {not_in_glb}"
        )

    merged: list[dict] = []
    for bone in base:
        merged.append({**bone, "parent_name": base_by[bone["name"]]["parent_name"]})
    extras = [b["name"] for b in extra if b["name"] in glb_set and b["name"] not in base_by]
    for name in extras:
        b = extra_by[name]
        merged.append(
            {"name": name, "pos": b["pos"], "quat": b["quat"], "parent_name": b["parent_name"]}
        )

    index_of = {b["name"]: i for i, b in enumerate(merged)}
    errors = []
    for bone in merged:
        pname = bone["parent_name"]
        if pname is not None and pname not in index_of:
            errors.append(f"{bone['name']}: parent {pname} not in merged set")
            continue
        gp = glb_parent.get(bone["name"])
        if gp in glb_set and gp != pname:
            errors.append(f"{bone['name']}: GLB parent {gp} != JSON parent {pname}")
    if errors:
        sys.exit("ERROR: hierarchy validation failed:\n  " + "\n  ".join(errors))

    out = [
        {
            "name": b["name"],
            "parent": index_of[b["parent_name"]] if b["parent_name"] is not None else -1,
            "pos": b["pos"],
            "quat": b["quat"],
        }
        for b in merged
    ]
    text = json.dumps(out, indent=1)
    if args.out:
        args.out.write_text(text)
    else:
        print(text)
    print(
        f"merged {len(base)} base + {len(extras)} extra = {len(out)} bones "
        f"(GLB rig: {len(glb_names)}); hierarchy validated",
        file=sys.stderr,
    )
    if len(out) != len(glb_names):
        sys.exit("ERROR: merged bone count != GLB joint count")


if __name__ == "__main__":
    main()
