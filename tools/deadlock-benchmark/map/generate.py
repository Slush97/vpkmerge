#!/usr/bin/env python3
"""Generate the historical particle lab sources, without touching the SDK stage."""
import argparse
import hashlib
import math
import random
import re
import uuid
from pathlib import Path

EFFECTS = [
    "particles/environment/plaza_fog_01_.vpcf",
    "particles/environment/street_steam_02.vpcf",
    "particles/environment/industrial_smoke_stack_thin.vpcf",
    "particles/environment/candle_flame.vpcf",
    "particles/endgame/endgame_amber_barrel_fire.vpcf",
    "particles/hideout/hideout_fireplace_fire_noshadows.vpcf",
    "particles/environmental/campus_fireflies_01.vpcf",
    "particles/environmental/street_light_glow_01.vpcf",
    "particles/environment/fan_wind.vpcf",
    "particles/environmental/storm_glow_01.vpcf",
]

# Floor area the emitters cover, inside the template room (x -448..448,
# y -384..576, z 0..448), and the camera that looks down it from the near wall.
GRID_X = (-400.0, 400.0)
GRID_Y = (-200.0, 540.0)
GRID_Z = 16.0
CAMERA = "0 -380 170 18 90"  # spec_goto x y z pitch yaw

ENTITY = """\
"CMapEntity"
{{
    "id" "elementid" "{id}"
    "nodeID" "int" "{node}"
    "referenceID" "uint64" "0x{ref:016x}"
    "children" "element_array" [ ]
    "variableTargetKeys" "string_array" [ ]
    "variableNames" "string_array" [ ]
    "relayPlugData" "DmePlugList"
    {{
        "id" "elementid" "{plug_id}"
        "names" "string_array" [ ]
        "dataTypes" "int_array" [ ]
        "plugTypes" "int_array" [ ]
        "descriptions" "string_array" [ ]
    }}
    "connectionsData" "element_array" [ ]
    "entity_properties" "EditGameClassProps"
    {{
        "id" "elementid" "{props_id}"
        "classname" "string" "info_particle_system"
        "vscripts" "string" ""
        "targetname" "string" "lab_emitter_{node}"
        "effect_name" "string" "{effect}"
        "start_active" "string" "1"
    }}
    "hitNormal" "vector3" "0 0 1"
    "isProceduralEntity" "bool" "0"
    "origin" "vector3" "{x:g} {y:g} {z:g}"
    "angles" "qangle" "0 0 0"
    "scales" "vector3" "1 1 1"
    "transformLocked" "bool" "0"
    "force_hidden" "bool" "0"
    "editorOnly" "bool" "0"
}}"""


def map_name(count: int) -> str:
    return f"dl_benchmark_lab_{count}"


def grid(count: int) -> list[tuple[float, float]]:
    if count == 0:
        return []
    w = GRID_X[1] - GRID_X[0]
    h = GRID_Y[1] - GRID_Y[0]
    cols = max(1, round(math.sqrt(count * w / h)))
    rows = math.ceil(count / cols)
    pts = []
    for i in range(count):
        r, c = divmod(i, cols)
        x = GRID_X[0] + w * (c + 0.5) / cols
        y = GRID_Y[0] + h * (r + 0.5) / rows
        pts.append((round(x, 2), round(y, 2)))
    return pts


def build_vmap(template: str, count: int) -> str:
    world = re.search(r'"world" "CMapWorld"\s*\{[^{]*?"children" "element_array"\s*\n\s*\[\n', template)
    if not world:
        raise ValueError("template has no CMapWorld children array")
    node = max(int(n) for n in re.findall(r'"nodeID" "int" "(\d+)"', template))
    rng = random.Random(count)
    blocks = []
    for i, (x, y) in enumerate(grid(count)):
        node += 1
        blocks.append(ENTITY.format(
            id=uuid.UUID(int=rng.getrandbits(128), version=4),
            plug_id=uuid.UUID(int=rng.getrandbits(128), version=4),
            props_id=uuid.UUID(int=rng.getrandbits(128), version=4),
            node=node,
            ref=rng.getrandbits(64),
            effect=EFFECTS[i % len(EFFECTS)],
            x=x, y=y, z=GRID_Z,
        ) + ",\n")
    at = world.end()
    return template[:at] + "".join(blocks) + template[at:]



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--template", type=Path, required=True, help="local CSDK volumetric_test.vmap in keyvalues2 format")
    parser.add_argument("--output", type=Path, required=True, help="new empty output directory")
    args = parser.parse_args()
    template = args.template.read_text(encoding="utf-8")
    if args.output.exists():
        parser.error("output must not already exist")
    args.output.mkdir(parents=True)
    for count in (0, 16):
        path = args.output / (map_name(count) + ".vmap")
        path.write_text(build_vmap(template, count), encoding="utf-8")
        print(f"{path}: sha256 {hashlib.sha256(path.read_bytes()).hexdigest()}")


if __name__ == "__main__":
    main()
