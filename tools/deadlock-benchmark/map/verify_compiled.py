#!/usr/bin/env python3
"""Inspect a compiled lab VPK; this does not prove that it renders in game."""
import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

from generate import EFFECTS, GRID_Z, grid


def modlock(*args):
    return subprocess.run(['modlock', *args], check=True, capture_output=True, text=True).stdout


def verify(archive: Path, count: int) -> None:
    name = f'dl_benchmark_lab_{count}'
    base = f'maps/{name}'
    listing = modlock('vpk', 'list', str(archive))
    entries = {}
    for line in listing.splitlines():
        match = re.fullmatch(r'\s*(\d+)\s+(.+)', line)
        if match:
            entries[match.group(2)] = int(match.group(1))
    required = [f'maps/{name}.vmap_c', f'{base}/entities/default_ents.vents_c',
                f'{base}/world.vwrld_c', f'{base}/world_physics.vmdl_c',
                f'{base}/worldnodes/n0.vwnod_c']
    missing = [entry for entry in required if entries.get(entry, 0) <= 0]
    if missing:
        raise ValueError(f'missing or empty compiled entries: {missing}')
    for entry, block in [(required[1], 'DATA'), (required[2], 'DATA'),
                         (required[3], 'PHYS'), (required[4], 'DATA')]:
        blocks = modlock('kv3', '--archive', str(archive), '--blocks', entry)
        if not re.search(rf'(?m)^\s*{block}\s+[1-9]\d*\s*$', blocks):
            raise ValueError(f'{entry}: missing or empty {block} block')
    data = json.loads(modlock('kv3', '--archive', str(archive), required[1]))
    values = [e['keyValues3Data']['values'] for e in data['m_entityKeyValues']]
    emitters = [e for e in values if e.get('classname') == 'info_particle_system']
    if len(emitters) != count:
        raise ValueError(f'expected {count} emitters, found {len(emitters)}')
    names = [e.get('targetname', '').split(']')[-1] for e in emitters]
    if any(not re.fullmatch(r'lab_emitter_\d+', name) for name in names) or len(set(names)) != count:
        raise ValueError('emitter target names missing or duplicated')
    expected = sorted((tuple((x, y, GRID_Z)), EFFECTS[i % len(EFFECTS)])
                      for i, (x, y) in enumerate(grid(count)))
    actual = sorted((tuple(e.get('origin', [])), e.get('effect_name')) for e in emitters)
    if actual != expected or any(e.get('start_active') not in (True, 1, '1') for e in emitters):
        raise ValueError('emitter positions, effect references or active flags differ from source')
    print(f'PASS structural inspection: {name}, {count} emitters, required resource blocks present')
    print('Not a runtime validation; check the current compiler log and render in a client.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=Path)
    parser.add_argument('--count', type=int, choices=(0, 16), required=True)
    args = parser.parse_args()
    try:
        verify(args.archive, args.count)
    except (OSError, subprocess.CalledProcessError, ValueError, KeyError, TypeError) as error:
        print(f'FAIL structural inspection: {error}', file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == '__main__':
    main()
