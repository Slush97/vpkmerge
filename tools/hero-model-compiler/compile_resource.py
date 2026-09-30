#!/usr/bin/env python3
"""Compile a single Source 2 source file (.vpcf / .vmat / ...) to its compiled
form via Valve's resourcecompiler under Proton, reusing the CSDK/Proton setup
from build_hero_model.py.

Stages <source> at content/citadel_addons/<addon>/<content_rel>, runs RC, and
reports the compiled output under game/citadel_addons/<addon>/<content_rel>_c.

Usage:
  compile_resource.py <source> <content_rel> [--addon NAME]
  e.g. compile_resource.py heart.vpcf particles/abilities/wraith/wraith_card_trick_heart.vpcf
"""
from __future__ import annotations
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

DEFAULT_CSDK_ROOT = Path(os.environ.get("CSDK_ROOT", "/home/esoc/csdk12/Reduced_CSDK_12"))
DEFAULT_PROTON = Path(os.environ.get(
    "PROTON",
    "/home/esoc/.local/share/Steam/steamapps/common/Proton - Experimental/proton"))
DEFAULT_STEAM_ROOT = Path(os.environ.get("STEAM_ROOT", "/home/esoc/.local/share/Steam"))
DEFAULT_PROTON_PREFIX = Path(os.environ.get("STEAM_COMPAT_DATA_PATH", "/tmp/proton-vpkmerge-rc"))


def wine_z_path(p: Path | str) -> str:
    return "Z:" + str(p).replace("/", "\\")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("source", type=Path)
    ap.add_argument("content_rel")
    ap.add_argument("--addon", default="shadow_wraith_cards")
    ap.add_argument("--csdk-root", type=Path, default=DEFAULT_CSDK_ROOT)
    ap.add_argument("--proton", type=Path, default=DEFAULT_PROTON)
    ap.add_argument("--proton-prefix", type=Path, default=DEFAULT_PROTON_PREFIX)
    ap.add_argument("--steam-root", type=Path, default=DEFAULT_STEAM_ROOT)
    args = ap.parse_args()

    csdk = args.csdk_root.resolve()
    content_addon = csdk / "content" / "citadel_addons" / args.addon
    game_addon = csdk / "game" / "citadel_addons" / args.addon
    compiler_dir = csdk / "game" / "bin_tools" / "win64"
    rc_exe = compiler_dir / "resourcecompiler.exe"
    for p in (rc_exe,):
        if not p.exists():
            sys.exit(f"missing: {p}")

    dest = content_addon / args.content_rel
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy(args.source.resolve(), dest)
    print(f"staged {args.source} -> {dest}")

    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False, encoding="utf-8") as f:
        filelist = Path(f.name)
        f.write(wine_z_path(dest) + "\n")

    args.proton_prefix.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.update({
        "STEAM_COMPAT_DATA_PATH": str(args.proton_prefix),
        "STEAM_COMPAT_CLIENT_INSTALL_PATH": str(args.steam_root),
        "SteamAppId": "1422450",
        "SteamGameId": "1422450",
        "VPROJECT": "1",
    })
    cmd = [str(args.proton), "run", wine_z_path(rc_exe),
           "-game", "citadel", "-addon", args.addon,
           "-fshallow", "-nop4", "-v", "-consoleapp", "-consolelog",
           "-condebug", "-toconsole", "-danger_mode_ignore_schema_mismatches",
           "-filelist", wine_z_path(filelist)]
    print("RC:", " ".join(cmd))
    try:
        r = subprocess.run(cmd, cwd=compiler_dir, env=env)
    finally:
        filelist.unlink(missing_ok=True)

    compiled = game_addon / (args.content_rel + "_c")
    print(f"\nrc exit {r.returncode}")
    if compiled.exists():
        print(f"COMPILED OK: {compiled} ({compiled.stat().st_size} bytes)")
        return 0
    print(f"NO OUTPUT at {compiled}")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
