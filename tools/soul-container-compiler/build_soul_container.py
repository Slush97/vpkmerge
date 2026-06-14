#!/usr/bin/env python3
"""Build a Source 2-compiled soul-container VPK from a GLB.

This is a Linux/Proton wrapper around Valve's resourcecompiler.exe. It prepares
source content with Blender, compiles through the Deadlock CSDK, and packs the
compiled game addon tree into a dir VPK.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
BLENDER_STAGE = HERE / "prepare_soul_container_blender.py"
DEFAULT_CSDK_ROOT = Path(os.environ.get("CSDK_ROOT", "/home/esoc/csdk12/Reduced_CSDK_12"))
DEFAULT_PROTON = Path(
    os.environ.get(
        "PROTON",
        "/home/esoc/.local/share/Steam/steamapps/common/Proton - Experimental/proton",
    )
)
DEFAULT_STEAM_ROOT = Path(os.environ.get("STEAM_ROOT", "/home/esoc/.local/share/Steam"))
DEFAULT_PROTON_PREFIX = Path(os.environ.get("STEAM_COMPAT_DATA_PATH", "/tmp/proton-vpkmerge-rc"))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Compile a GLB into a Source 2 soul-container addon VPK.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    parser.add_argument("input_glb", type=Path)
    parser.add_argument("--addon", default="soul_container_import")
    parser.add_argument("--output", type=Path, default=None)
    parser.add_argument("--model-rel", default="models/props_gameplay/soul_container")
    parser.add_argument("--target-largest-axis", type=float, default=12.65)
    parser.add_argument("--source-units-per-blender", type=float, default=100.0)
    parser.add_argument("--physics-radius", type=float, default=7.0)
    parser.add_argument("--csdk-root", type=Path, default=DEFAULT_CSDK_ROOT)
    parser.add_argument("--proton", type=Path, default=DEFAULT_PROTON)
    parser.add_argument("--steam-root", type=Path, default=DEFAULT_STEAM_ROOT)
    parser.add_argument("--proton-prefix", type=Path, default=DEFAULT_PROTON_PREFIX)
    parser.add_argument("--blender", default=os.environ.get("BLENDER", "blender"))
    parser.add_argument("--force", action="store_true", help="remove existing CSDK addon staging dirs")
    parser.add_argument("--keep-staging", action="store_true", help="keep generated content/game staging dirs")
    parser.add_argument("--install-to", type=Path, default=None, help="optional exact pakXX_dir.vpk install path")
    parser.add_argument("--metadata", type=Path, default=None, help="optional Grimoire mod-metadata.json to update")
    return parser.parse_args()


def run(cmd: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None) -> None:
    display = " ".join(cmd)
    print(f"+ {display}", flush=True)
    subprocess.run(cmd, cwd=cwd, env=env, check=True)


def wine_z_path(path: Path) -> str:
    return "Z:" + str(path.resolve()).replace("/", "\\")


def validate_addon_name(addon: str) -> None:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", addon):
        raise SystemExit(f"addon name must be file-name safe, got: {addon!r}")
    if addon in {".", ".."}:
        raise SystemExit(f"invalid addon name: {addon!r}")


def require_file(path: Path, label: str) -> None:
    if not path.is_file():
        raise SystemExit(f"{label} not found: {path}")


def require_dir(path: Path, label: str) -> None:
    if not path.is_dir():
        raise SystemExit(f"{label} not found: {path}")


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def remove_staging(path: Path, *, force: bool) -> None:
    if not path.exists():
        return
    if not force:
        raise SystemExit(f"staging path exists; rerun with --force to remove it: {path}")
    shutil.rmtree(path)


def update_metadata(metadata_path: Path, installed_vpk: Path, digest: str) -> None:
    if not metadata_path.exists():
        raise SystemExit(f"metadata file not found: {metadata_path}")

    data = json.loads(metadata_path.read_text(encoding="utf-8"))
    key = installed_vpk.name
    if key not in data:
        data[key] = {
            "modName": installed_vpk.stem.replace("_dir", "").replace("_", " ").title(),
            "sourceFileName": installed_vpk.stem,
            "globalType": "soul-container",
            "globalTypeClassifierVersion": 1,
            "abilitySounds": None,
            "nsfw": False,
        }
    data[key]["sha256"] = digest

    tmp = metadata_path.with_suffix(metadata_path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    tmp.replace(metadata_path)


def main() -> int:
    args = parse_args()
    validate_addon_name(args.addon)

    input_glb = args.input_glb.resolve()
    csdk_root = args.csdk_root.resolve()
    output = (args.output or (REPO_ROOT / "target" / f"{args.addon}_dir.vpk")).resolve()
    content_addon = csdk_root / "content" / "citadel_addons" / args.addon
    game_addon = csdk_root / "game" / "citadel_addons" / args.addon
    source_dir = content_addon / args.model_rel
    compiler_dir = csdk_root / "game" / "bin_tools" / "win64"

    require_file(input_glb, "input GLB")
    require_file(args.proton, "Proton executable")
    require_file(BLENDER_STAGE, "Blender stage script")
    require_dir(compiler_dir, "resourcecompiler directory")

    remove_staging(content_addon, force=args.force)
    remove_staging(game_addon, force=args.force)
    source_dir.mkdir(parents=True, exist_ok=True)
    output.parent.mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    env["S2_MODEL_REL"] = args.model_rel
    env["S2_TARGET_LARGEST_AXIS"] = str(args.target_largest_axis)
    env["S2_SOURCE_UNITS_PER_BLENDER"] = str(args.source_units_per_blender)
    env["S2_PHYSICS_RADIUS"] = str(args.physics_radius)

    run(
        [
            args.blender,
            "--background",
            "--python",
            str(BLENDER_STAGE),
            "--",
            str(input_glb),
            str(source_dir),
        ],
        env=env,
    )

    source_vmdl = source_dir / "soul_container.vmdl"
    require_file(source_vmdl, "generated source model")

    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False, encoding="utf-8") as f:
        filelist = Path(f.name)
        f.write(wine_z_path(source_vmdl) + "\n")

    rc_env = os.environ.copy()
    rc_env.update(
        {
            "STEAM_COMPAT_DATA_PATH": str(args.proton_prefix),
            "STEAM_COMPAT_CLIENT_INSTALL_PATH": str(args.steam_root),
            "SteamAppId": "1422450",
            "SteamGameId": "1422450",
            "VPROJECT": "1",
        }
    )

    try:
        run(
            [
                str(args.proton),
                "run",
                "resourcecompiler.exe",
                "-game",
                "citadel",
                "-addon",
                args.addon,
                "-fshallow",
                "-nop4",
                "-v",
                "-consoleapp",
                "-consolelog",
                "-condebug",
                "-toconsole",
                "-danger_mode_ignore_schema_mismatches",
                "-filelist",
                wine_z_path(filelist),
            ],
            cwd=compiler_dir,
            env=rc_env,
        )
    finally:
        filelist.unlink(missing_ok=True)

    require_dir(game_addon, "compiled game addon")
    run(
        [
            "cargo",
            "run",
            "--release",
            "-p",
            "vpkmerge-core",
            "--example",
            "pack_tree",
            "--",
            str(game_addon),
            str(output),
        ],
        cwd=REPO_ROOT,
    )

    digest = sha256(output)
    print(f"built {output}")
    print(f"sha256 {digest}")

    if args.install_to:
        install_to = args.install_to.resolve()
        install_to.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(output, install_to)
        digest = sha256(install_to)
        print(f"installed {install_to}")
        print(f"installed_sha256 {digest}")
        if args.metadata:
            update_metadata(args.metadata.resolve(), install_to, digest)
            print(f"updated metadata {args.metadata.resolve()}")

    if not args.keep_staging:
        shutil.rmtree(content_addon, ignore_errors=True)
        shutil.rmtree(game_addon, ignore_errors=True)

    return 0


if __name__ == "__main__":
    sys.exit(main())
