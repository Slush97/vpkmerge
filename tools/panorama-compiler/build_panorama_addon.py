#!/usr/bin/env python3
"""Compile a hand-authored Panorama source tree into a Deadlock addon VPK.

Linux/Proton wrapper around Valve's resourcecompiler.exe (the same CSDK harness
the soul-container compiler uses). Stages a `src/panorama/...` tree under the
CSDK content addon, compiles every .vxml/.vjs/.vcss/.vsvg to its `_c` form, then
packs the compiled game addon tree into a dir VPK.

Usage:
    tools/panorama-compiler/build_panorama_addon.py grimhud/src \
        --addon grimhud_probe --output target/grimhud_probe_dir.vpk --force
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CSDK_ROOT = Path(os.environ.get("CSDK_ROOT", "/home/esoc/csdk12/Reduced_CSDK_12"))
DEFAULT_PROTON = Path(
    os.environ.get(
        "PROTON",
        "/home/esoc/.local/share/Steam/steamapps/common/Proton - Experimental/proton",
    )
)
DEFAULT_STEAM_ROOT = Path(os.environ.get("STEAM_ROOT", "/home/esoc/.local/share/Steam"))
DEFAULT_PROTON_PREFIX = Path(os.environ.get("STEAM_COMPAT_DATA_PATH", "/tmp/proton-vpkmerge-rc"))

# Panorama source extensions resourcecompiler knows how to compile.
COMPILE_EXTS = {".vxml", ".vjs", ".vcss", ".vsvg"}
# Asset extensions copied through as-is (already-compiled resources, fonts, etc.).
PASSTHROUGH_EXTS = {".vtex_c", ".vsnd_c", ".vsndevts_c", ".ttf", ".otf", ".vxml_c", ".vjs_c", ".vcss_c", ".vsvg_c"}


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="Compile a Panorama source tree into a Deadlock addon VPK.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    p.add_argument("src", type=Path, help="source root containing a panorama/ subtree")
    p.add_argument("--addon", default="grimhud_probe")
    p.add_argument("--output", type=Path, default=None)
    p.add_argument("--csdk-root", type=Path, default=DEFAULT_CSDK_ROOT)
    p.add_argument("--proton", type=Path, default=DEFAULT_PROTON)
    p.add_argument("--steam-root", type=Path, default=DEFAULT_STEAM_ROOT)
    p.add_argument("--proton-prefix", type=Path, default=DEFAULT_PROTON_PREFIX)
    p.add_argument("--force", action="store_true", help="remove existing CSDK addon staging dirs")
    p.add_argument("--keep-staging", action="store_true", help="keep generated content/game staging dirs")
    return p.parse_args()


def run(cmd: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None) -> None:
    print("+ " + " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, env=env, check=True)


def wine_z_path(path: Path) -> str:
    return "Z:" + str(path.resolve()).replace("/", "\\")


def validate_addon_name(addon: str) -> None:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", addon) or addon in {".", ".."}:
        raise SystemExit(f"addon name must be file-name safe, got: {addon!r}")


def require_file(path: Path, label: str) -> None:
    if not path.is_file():
        raise SystemExit(f"{label} not found: {path}")


def require_dir(path: Path, label: str) -> None:
    if not path.is_dir():
        raise SystemExit(f"{label} not found: {path}")


def remove_staging(path: Path, *, force: bool) -> None:
    if not path.exists():
        return
    if not force:
        raise SystemExit(f"staging path exists; rerun with --force to remove it: {path}")
    shutil.rmtree(path)


def main() -> int:
    args = parse_args()
    validate_addon_name(args.addon)

    src = args.src.resolve()
    src_panorama = src / "panorama"
    csdk_root = args.csdk_root.resolve()
    output = (args.output or (REPO_ROOT / "target" / f"{args.addon}_dir.vpk")).resolve()
    content_addon = csdk_root / "content" / "citadel_addons" / args.addon
    game_addon = csdk_root / "game" / "citadel_addons" / args.addon
    compiler_dir = csdk_root / "game" / "bin_tools" / "win64"

    require_dir(src_panorama, "source panorama/ tree")
    require_file(args.proton, "Proton executable")
    require_dir(compiler_dir, "resourcecompiler directory")

    remove_staging(content_addon, force=args.force)
    remove_staging(game_addon, force=args.force)
    content_addon.mkdir(parents=True, exist_ok=True)
    output.parent.mkdir(parents=True, exist_ok=True)

    # Stage the source tree under content/citadel_addons/<addon>/panorama/...
    compile_list: list[Path] = []
    for f in sorted(src_panorama.rglob("*")):
        if not f.is_file():
            continue
        rel = f.relative_to(src)
        dest = content_addon / rel
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(f, dest)
        if f.suffix in COMPILE_EXTS:
            compile_list.append(dest)
        elif f.suffix not in PASSTHROUGH_EXTS and "".join(f.suffixes[-1:]) not in PASSTHROUGH_EXTS:
            print(f"  note: copied through unrecognized asset {rel}", flush=True)

    if not compile_list:
        raise SystemExit("no compilable Panorama source files (.vxml/.vjs/.vcss/.vsvg) found")

    print(f"staged {len(compile_list)} source file(s) for compile:")
    for c in compile_list:
        print("  " + str(c.relative_to(content_addon)))

    # resourcecompiler reads a newline-delimited filelist of wine Z: paths.
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False, encoding="utf-8") as fh:
        filelist = Path(fh.name)
        for c in compile_list:
            fh.write(wine_z_path(c) + "\n")

    # Proton needs the compatdata dir to exist before it initializes the prefix
    # (it lives under /tmp by default, which is wiped on reboot).
    args.proton_prefix.mkdir(parents=True, exist_ok=True)

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
                # Absolute path: Proton's `run` cannot resolve a bare exe name even
                # with cwd set (fails with "Failed to create process: 2").
                str(compiler_dir / "resourcecompiler.exe"),
                "-game", "citadel",
                "-addon", args.addon,
                "-fshallow",
                "-nop4",
                "-v",
                "-consoleapp",
                "-consolelog",
                "-condebug",
                "-toconsole",
                "-danger_mode_ignore_schema_mismatches",
                "-filelist", wine_z_path(filelist),
            ],
            cwd=compiler_dir,
            env=rc_env,
        )
    finally:
        filelist.unlink(missing_ok=True)

    require_dir(game_addon, "compiled game addon")
    compiled = sorted(p.relative_to(game_addon) for p in game_addon.rglob("*") if p.is_file())
    print(f"\nresourcecompiler produced {len(compiled)} file(s):")
    for c in compiled:
        print("  " + str(c))

    run(
        ["cargo", "run", "--release", "-p", "vpkmerge-core", "--example", "pack_tree",
         "--", str(game_addon), str(output)],
        cwd=REPO_ROOT,
    )
    print(f"\nbuilt {output}")

    if not args.keep_staging:
        shutil.rmtree(content_addon, ignore_errors=True)
        shutil.rmtree(game_addon, ignore_errors=True)

    return 0


if __name__ == "__main__":
    sys.exit(main())
