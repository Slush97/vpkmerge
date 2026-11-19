#!/usr/bin/env python3
"""Read-only inventory; a matching version does not establish runtime compatibility."""
import argparse
import hashlib
import json
import shutil
import subprocess
from pathlib import Path

CSDK13_FILES = ('read_me.txt', 'csdkcfg.exe', 'game/citadel/steam.inf',
                'game/citadel/gameinfo.gi', 'game/bin/win64/resourcecompiler.exe',
                'game/bin/win64/resourcecompiler.dll', 'game/bin/win64/engine2.dll',
                'game/bin/win64/tier0.dll', 'game/bin/win64/schemasystem.dll',
                'game/bin/win64/meshsystem.dll', 'game/bin/win64/modeldoc_utils.dll')


def fields(path):
    try:
        return dict(line.split('=', 1) for line in path.read_text().splitlines() if '=' in line)
    except OSError:
        return {}


def digest(path):
    try:
        with path.open('rb') as f:
            return hashlib.file_digest(f, 'sha256').hexdigest()
    except OSError:
        return None


def version(executable, args):
    if not executable:
        return None
    try:
        p = subprocess.run([executable, *args], capture_output=True, text=True, timeout=5, check=False)
        return (p.stdout or p.stderr).strip() if p.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--game', type=Path, required=True, help='Deadlock installation root (containing game/)')
    p.add_argument('--stage', type=Path, required=True, help='CSDK stage root')
    p.add_argument('--profile', choices=('retail-mixed', 'csdk13'), default='retail-mixed',
                   help='toolchain inventory profile (default: retail-mixed)')
    p.add_argument('--map', type=Path, help='Existing compiled map VPK to fingerprint')
    args = p.parse_args()
    retail = fields(args.game / 'game/citadel/steam.inf')
    stamp = fields(args.stage / '.stage-stamp')
    staged = fields(args.stage / 'game/citadel/steam.inf')
    expected = (retail.get('ClientVersion'), retail.get('SourceRevision'))
    actual = (stamp.get('CLIENT_VERSION') or staged.get('ClientVersion'),
              stamp.get('SOURCE_REVISION') or staged.get('SourceRevision'))
    match = all(expected) and all(actual) and expected == actual
    cohort = {name: digest(args.stage / name) for name in CSDK13_FILES} if args.profile == 'csdk13' else {}
    missing = [name for name, sha in cohort.items() if sha is None]
    try:
        marker = 'Reduced CSDK 13' in (args.stage / 'read_me.txt').read_text(encoding='utf-8', errors='replace')
    except OSError:
        marker = False
    inventory_complete = not missing and marker
    modlock, mangohud = shutil.which('modlock'), shutil.which('mangohud')
    report = {
        'profile': args.profile, 'game': str(args.game), 'stage': str(args.stage),
        'game_build': expected[0], 'game_revision': expected[1],
        'stage_build': actual[0], 'stage_revision': actual[1],
        'versions_match': bool(match),
        'resourcecompiler_present': (args.stage / 'game/bin/win64/resourcecompiler.exe').is_file(),
        'modlock': {'path': modlock, 'version': version(modlock, ['--version'])},
        'mangohud': {'path': mangohud, 'version': version(mangohud, ['--version'])},
        'map': {'path': str(args.map), 'sha256': digest(args.map)} if args.map else None,
        'compiler_inventory': ({'complete': inventory_complete, 'missing': missing,
                                'package_marker_present': marker, 'sha256': cohort}
                               if args.profile == 'csdk13' else None),
        'runtime_compatibility': 'unverified',
        'compiler_output_compatibility': 'unverified',
    }
    print(json.dumps(report, indent=2))
    return 0 if (inventory_complete if args.profile == 'csdk13' else (match and report['resourcecompiler_present'])) else 1


if __name__ == '__main__':
    raise SystemExit(main())
