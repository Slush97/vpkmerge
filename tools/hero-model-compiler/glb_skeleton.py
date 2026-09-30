#!/usr/bin/env python3
"""Emit a dump_skeleton-shaped JSON from a GLB's skin rig, in Source space.

Why this exists: RC binds the staged FBX mesh to the FBX (= GLB) armature's
rest pose, while the engine attaches the mesh through the emitted .vmdl
skeleton's binds. Any bone whose emitted bind differs from the GLB rest pose
displaces its geometry rigidly in-game (the Baroness Mina kitty floated 15-20
units forward when its bones were transplanted from another mod's skeleton).
Emitting the skeleton FROM the GLB makes the two identical by construction.

The GLB joint locals are composed through the scene (including the armature
node's Y-up->Z-up rotation), giving Source-space world transforms; each bone's
stored pos/quat is then parent-relative within the joint set (world for roots).

A reference dump_skeleton JSON (e.g. the donor mod's) provides the output BONE
ORDER for its shared bones (extras appended in GLB order) and a sanity gate:
a shared bone whose world bind drifts beyond --warn/--max units is reported /
fails the build, catching axis-convention bugs deterministically.

Usage: glb_skeleton.py <rig.glb> <ref_skel.json> [-o out.json]
       [--warn 2.0] [--max 8.0]
"""

from __future__ import annotations

import argparse
import json
import math
import struct
import sys
from pathlib import Path


def load_glb(path: Path) -> dict:
    data = path.read_bytes()
    magic, _ver, _length = struct.unpack("<III", data[:12])
    if magic != 0x46546C67:
        sys.exit(f"{path}: not a GLB")
    json_len, chunk_type = struct.unpack("<II", data[12:20])
    if chunk_type != 0x4E4F534A:
        sys.exit(f"{path}: first chunk is not JSON")
    return json.loads(data[20 : 20 + json_len])


def quat_mat(q):
    x, y, z, w = q
    return [
        [1 - 2 * y * y - 2 * z * z, 2 * x * y - 2 * w * z, 2 * x * z + 2 * w * y],
        [2 * x * y + 2 * w * z, 1 - 2 * x * x - 2 * z * z, 2 * y * z - 2 * w * x],
        [2 * x * z - 2 * w * y, 2 * y * z + 2 * w * x, 1 - 2 * x * x - 2 * y * y],
    ]


def mat_mul(a, b):
    return [[sum(a[i][k] * b[k][j] for k in range(3)) for j in range(3)] for i in range(3)]


def mat_vec(a, v):
    return [sum(a[i][k] * v[k] for k in range(3)) for i in range(3)]


def mat_t(a):
    return [[a[j][i] for j in range(3)] for i in range(3)]


def mat_quat(m):
    """Rotation matrix -> quaternion [x, y, z, w] (Shepperd's method)."""
    tr = m[0][0] + m[1][1] + m[2][2]
    if tr > 0:
        s = math.sqrt(tr + 1.0) * 2
        return [
            (m[2][1] - m[1][2]) / s,
            (m[0][2] - m[2][0]) / s,
            (m[1][0] - m[0][1]) / s,
            0.25 * s,
        ]
    if m[0][0] > m[1][1] and m[0][0] > m[2][2]:
        s = math.sqrt(1.0 + m[0][0] - m[1][1] - m[2][2]) * 2
        return [
            0.25 * s,
            (m[0][1] + m[1][0]) / s,
            (m[0][2] + m[2][0]) / s,
            (m[2][1] - m[1][2]) / s,
        ]
    if m[1][1] > m[2][2]:
        s = math.sqrt(1.0 + m[1][1] - m[0][0] - m[2][2]) * 2
        return [
            (m[0][1] + m[1][0]) / s,
            0.25 * s,
            (m[1][2] + m[2][1]) / s,
            (m[0][2] - m[2][0]) / s,
        ]
    s = math.sqrt(1.0 + m[2][2] - m[0][0] - m[1][1]) * 2
    return [
        (m[0][2] + m[2][0]) / s,
        (m[1][2] + m[2][1]) / s,
        0.25 * s,
        (m[1][0] - m[0][1]) / s,
    ]


def world_of_ref(skel):
    worlds = {}

    def compute(i):
        b = skel[i]
        if b["name"] in worlds:
            return worlds[b["name"]]
        rot = quat_mat(b["quat"])
        pos = list(b["pos"])
        if b["parent"] >= 0:
            pr, pt = compute(b["parent"])
            rot2 = mat_mul(pr, rot)
            pos2 = [a + c for a, c in zip(mat_vec(pr, pos), pt)]
        else:
            rot2, pos2 = rot, pos
        worlds[b["name"]] = (rot2, pos2)
        return worlds[b["name"]]

    for i in range(len(skel)):
        compute(i)
    return worlds


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("glb", type=Path)
    ap.add_argument("ref_json", type=Path)
    ap.add_argument("-o", "--out", type=Path, default=None)
    ap.add_argument("--warn", type=float, default=2.0)
    ap.add_argument("--max", type=float, default=8.0)
    args = ap.parse_args()

    gltf = load_glb(args.glb)
    nodes = gltf["nodes"]
    skins = gltf.get("skins") or []
    if not skins:
        sys.exit("GLB has no skin")
    joints = skins[0]["joints"]
    joint_set = set(joints)

    parent_node = {}
    for i, node in enumerate(nodes):
        for ch in node.get("children", []):
            parent_node[ch] = i

    world_cache: dict[int, tuple] = {}

    def node_world(n):
        if n in world_cache:
            return world_cache[n]
        node = nodes[n]
        rot = quat_mat(node.get("rotation", [0, 0, 0, 1]))
        pos = list(node.get("translation", [0.0, 0.0, 0.0]))
        scale = node.get("scale", [1, 1, 1])
        if any(abs(s - 1.0) > 1e-6 for s in scale):
            sys.exit(f"joint node {node.get('name')} carries scale {scale}; unsupported")
        if n in parent_node:
            pr, pt = node_world(parent_node[n])
            world_cache[n] = (mat_mul(pr, rot), [a + c for a, c in zip(mat_vec(pr, pos), pt)])
        else:
            world_cache[n] = (rot, pos)
        return world_cache[n]

    name_of = {j: nodes[j].get("name", f"node{j}") for j in joints}
    glb_world = {name_of[j]: node_world(j) for j in joints}
    glb_parent = {}
    for j in joints:
        p = parent_node.get(j)
        glb_parent[name_of[j]] = name_of[p] if p in joint_set else None

    ref = json.loads(args.ref_json.read_text())
    ref_names = [b["name"] for b in ref]
    ref_world = world_of_ref(ref)

    # Output order: ref order for bones the GLB carries, then GLB-only extras
    # in joint order.
    order = [n for n in ref_names if n in glb_world]
    order += [name_of[j] for j in joints if name_of[j] not in set(order)]

    # Sanity gate: shared-bone world binds should be close to the ref's.
    drift = []
    for n in order:
        if n in ref_world:
            d = math.dist(glb_world[n][1], ref_world[n][1])
            if d > args.warn:
                drift.append((n, d))
    hard = [(n, d) for n, d in drift if d > args.max]
    if hard:
        sys.exit(
            "ERROR: shared-bone world bind drift exceeds --max "
            f"(axis convention bug?): {[(n, round(d, 2)) for n, d in hard]}"
        )
    for n, d in drift:
        print(f"note: {n} drifts {d:.2f} units from ref (GLB rest wins)", file=sys.stderr)

    index_of = {n: i for i, n in enumerate(order)}
    out = []
    for n in order:
        rot_w, pos_w = glb_world[n]
        pname = glb_parent[n]
        if pname is None:
            rel_r, rel_p = rot_w, pos_w
            parent = -1
        else:
            pr, pt = glb_world[pname]
            inv = mat_t(pr)
            rel_r = mat_mul(inv, rot_w)
            rel_p = mat_vec(inv, [a - b for a, b in zip(pos_w, pt)])
            parent = index_of[pname]
            if parent >= index_of[n]:
                sys.exit(f"ERROR: bone {n} precedes its parent {pname} in output order")
        out.append({"name": n, "parent": parent, "pos": rel_p, "quat": mat_quat(rel_r)})

    text = json.dumps(out, indent=1)
    if args.out:
        args.out.write_text(text)
    else:
        print(text)
    print(
        f"emitted {len(out)} bones from GLB rig ({len(order) - len(glb_world)} missing?); "
        f"{len(drift)} shared bones drift >{args.warn} from ref",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
