#!/usr/bin/env python3
"""Inject grimhud <styles>/<scripts> includes into a decompiled hud.vxml.

The production HUD-injection transform: take whatever hud.vxml the user's CURRENT
game ships (decompile it with `morphic-oracle panorama decompile`), add our style
+ script includes, and emit a source hud.vxml ready to recompile. Re-derive this
on every Deadlock patch so the override never reverts Valve's HUD changes.

Usage:
    inject_hud_scripts.py <decompiled_hud.vxml> <out_hud.vxml> \
        --style panorama/styles/grimhud_probe.vcss_c \
        --script panorama/scripts/grimhud_probe.vjs_c [--script ...]
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("src", type=Path, help="decompiled source hud.vxml")
    p.add_argument("out", type=Path, help="output injected hud.vxml")
    p.add_argument("--style", action="append", default=[], help="panorama/.../x.vcss_c to include (repeatable)")
    p.add_argument("--script", action="append", default=[], help="panorama/.../x.vjs_c to include (repeatable)")
    return p.parse_args()


def main() -> int:
    args = parse_args()
    text = args.src.read_text(encoding="utf-8")

    if "</styles>" not in text:
        print("error: no <styles> block found in source hud.vxml", file=sys.stderr)
        return 2

    indent = "\t\t"
    # Inject style includes just before </styles> so our overrides win cascade order.
    if args.style:
        style_lines = "".join(
            f'{indent}<include src="s2r://{s}" />\n' for s in args.style
        )
        text = text.replace("\t</styles>", style_lines + "\t</styles>", 1)

    # Add a <scripts> block right after </styles> (the game HUD ships none).
    if args.script:
        script_lines = "".join(
            f'{indent}<include src="s2r://{s}" />\n' for s in args.script
        )
        scripts_block = "\t<scripts>\n" + script_lines + "\t</scripts>\n"
        text = text.replace("\t</styles>\n", "\t</styles>\n" + scripts_block, 1)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(text, encoding="utf-8")
    print(f"wrote {args.out} (+{len(args.style)} style, +{len(args.script)} script include(s))")
    return 0


if __name__ == "__main__":
    sys.exit(main())
