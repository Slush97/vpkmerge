#!/usr/bin/env python3
"""Attach local retail VPKs to an independently extracted CSDK13 stage."""
import argparse
import hashlib
import json
import shutil
from pathlib import Path


def digest(p):
    with p.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--stage', required=True, type=Path, help='dedicated extracted CSDK13 directory')
    ap.add_argument('--retail-game', required=True, type=Path, help='installed Deadlock game directory')
    ap.add_argument('--acknowledge-isolated-stage', required=True, action='store_true')
    args = ap.parse_args()
    stage = args.stage.resolve(strict=True)
    retail = args.retail_game.resolve(strict=True)
    if stage == retail or stage in retail.parents or retail in stage.parents:
        ap.error('stage and retail game must be separate directories')
    if (stage / 'game').is_symlink():
        ap.error('stage game directory must not be a symlink')
    if not (stage / 'game/bin/win64/resourcecompiler.exe').is_file():
        ap.error('stage is missing resourcecompiler.exe')
    for name in ('citadel', 'core'):
        dst = stage / 'game' / name
        if dst.is_symlink() or not dst.is_dir() or not (retail / name).is_dir():
            ap.error(f'missing or symlinked stage game/{name}, or missing retail assets')
    original = stage / 'game/citadel/readonly_tools_asset_info.binOFF'
    enabled = original.with_name('readonly_tools_asset_info.bin')
    if not original.is_file() or original.is_symlink():
        ap.error('stage lacks original readonly_tools_asset_info.binOFF')
    if enabled.is_symlink() or (enabled.exists() and digest(enabled) != digest(original)):
        ap.error('enabled asset info exists with different content or is a symlink')
    links = []
    for name in ('citadel', 'core'):
        for pattern in ('pak01_*.vpk', 'shaders_*.vpk'):
            for source in sorted((retail / name).glob(pattern)):
                dest = stage / 'game' / name / source.name
                if dest.is_symlink():
                    if dest.resolve() != source.resolve():
                        ap.error(f'conflicting asset symlink: {dest}')
                elif dest.exists():
                    continue  # preserve a file already supplied by the package
                else:
                    links.append((source, dest))
    for source, dest in links:
        dest.symlink_to(source)
    if not enabled.exists():
        shutil.copyfile(original, enabled)
    print(json.dumps({'stage': str(stage), 'retail_game': str(retail), 'new_asset_links': len(links),
                      'asset_info_sha256': digest(enabled),
                      'stage_game_version': (stage / 'game/citadel/steam.inf').read_text(errors='replace'),
                      'retail_game_version': (retail / 'citadel/steam.inf').read_text(errors='replace'),
                      'compatibility': 'unverified'}))


if __name__ == '__main__':
    main()
