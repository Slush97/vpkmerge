#!/usr/bin/env python3
"""Compile lab VMAPs in an already prepared, isolated CSDK13 stage.

Retains a unique addon, Proton prefix, compiler logs and a JSON manifest. This
invokes third-party native code and requires an explicitly isolated stage.
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import uuid
from pathlib import Path

NAMES = ('dl_benchmark_lab_0', 'dl_benchmark_lab_16')


def sha(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def win(path):
    return 'Z:' + str(path.resolve()).replace('/', '\\')


def plain_dirs(root, relative):
    p = root
    for part in relative.parts:
        p = p / part
        if p.is_symlink():
            raise ValueError(f'write directory is a symlink: {p}')
        p.mkdir(exist_ok=True)
    return p


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--stage', required=True, type=Path)
    p.add_argument('--sources', required=True, type=Path)
    p.add_argument('--proton', required=True, type=Path)
    p.add_argument('--steam', required=True, type=Path, help='Steam client installation')
    p.add_argument('--results', required=True, type=Path, help='new, empty path for logs, manifest and dedicated prefix')
    p.add_argument('--acknowledge-isolated-stage', action='store_true', required=True,
                   help='confirm the stage is a dedicated writable copy, not retail or a shared SDK')
    p.add_argument('--archive-sha256', help='SHA-256 of the downloaded toolchain archive, if known')
    args = p.parse_args()
    stage = args.stage.resolve(strict=True)
    proton = args.proton.resolve(strict=True)
    steam = args.steam.resolve(strict=True)
    binary = stage / 'game/bin/win64/resourcecompiler.exe'
    sources = [args.sources / (name + '.vmap') for name in NAMES]
    if not binary.is_file() or not all(x.is_file() for x in sources):
        p.error('missing resourcecompiler.exe or one of the two generated VMAPs')
    if not (stage / 'game/citadel/readonly_tools_asset_info.bin').is_file():
        p.error('stage lacks enabled citadel readonly_tools_asset_info.bin')
    if args.archive_sha256 and not re.fullmatch(r'[0-9a-fA-F]{64}', args.archive_sha256):
        p.error('archive hash must be 64 hex digits')
    results = args.results.absolute()
    results.mkdir(parents=True, exist_ok=False)
    addon = 'benchmark_' + uuid.uuid4().hex[:16]
    srcroot = plain_dirs(stage, Path('content/citadel_addons'))
    outroot = plain_dirs(stage, Path('game/citadel_addons'))
    srcaddon, outaddon = srcroot / addon, outroot / addon
    srcaddon.mkdir()
    outaddon.mkdir()
    (srcaddon / 'maps').mkdir()
    (outaddon / 'maps').mkdir()
    prefix = results / 'proton-prefix'
    prefix.mkdir()
    cohort_paths = ['game/bin/win64/resourcecompiler.exe', 'game/bin/win64/resourcecompiler.dll',
                    'game/bin/win64/tier0.dll', 'game/bin/win64/modeldoc_utils.dll',
                    'game/citadel/readonly_tools_asset_info.bin', 'game/citadel/steam.inf']
    report = {'stage': str(stage), 'addon': addon, 'archive_sha256': args.archive_sha256,
              'cohort_sha256': {x: sha(stage / x) if (stage / x).is_file() else None for x in cohort_paths},
              'stage_steam_inf': (stage / 'game/citadel/steam.inf').read_text(errors='replace'),
              'flags': ['-world', '-phys', '-nolightmaps', '-threads', '8'],
              'runtime_compatibility': 'unverified', 'runs': []}
    def save():
        (results / 'manifest.json').write_text(json.dumps(report, indent=2) + '\n')
    save()
    env = os.environ.copy()
    env.update(STEAM_COMPAT_DATA_PATH=str(prefix), STEAM_COMPAT_CLIENT_INSTALL_PATH=str(steam),
               SteamAppId='1422450', SteamGameId='1422450', VPROJECT='1', WINEDEBUG='-all')
    for name, source in zip(NAMES, sources):
        copied = srcaddon / 'maps' / source.name
        shutil.copyfile(source, copied)
        logfile = results / (name + '.log')
        filelist = results / (name + '.filelist.txt')
        filelist.write_bytes((win(copied) + '\r\n').encode())
        output = outaddon / 'maps' / (name + '.vpk')
        cmd = [str(proton), 'run', win(binary), '-game', 'citadel', '-addon', addon, '-fshallow', '-nop4', '-v',
               '-consoleapp', '-con_logfile', win(logfile), '-toconsole',
               '-world', '-phys', '-nolightmaps', '-threads', '8', '-filelist', win(filelist)]
        entry = {'map': name, 'source_sha256': sha(copied), 'log': str(logfile), 'output': str(output)}
        report['runs'].append(entry)
        save()
        with (results / (name + '.stderr')).open('wb') as stderr:
            proc = subprocess.Popen(cmd, cwd=binary.parent, env=env, stdout=stderr, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                status = proc.wait(timeout=180)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGTERM)
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(proc.pid, signal.SIGKILL)
                    proc.wait()
                wineserver = proton.parent / 'files/bin/wineserver'
                if wineserver.is_file() and (prefix / 'pfx').is_dir():
                    cleanup_env = env.copy()
                    cleanup_env['WINEPREFIX'] = str(prefix / 'pfx')
                    try:
                        subprocess.run([str(wineserver), '-k'], env=cleanup_env, timeout=10, check=False)
                    except (OSError, subprocess.TimeoutExpired):
                        entry['cleanup_warning'] = 'owned prefix cleanup failed'
                else:
                    entry['cleanup_warning'] = 'could not locate wineserver or owned prefix'
                entry['error'] = 'compiler timed out'
                save()
                return 1
        text = logfile.read_text(encoding='utf-8-sig', errors='replace') if logfile.is_file() else ''
        summaries = re.findall(r'(\d+) compiled, (\d+) failed', text)
        success = status == 0 and bool(summaries) and int(summaries[-1][0]) > 0 and int(summaries[-1][1]) == 0 and output.is_file() and output.stat().st_size > 0
        entry.update(exit_code=status, summary=summaries[-1] if summaries else None,
                     output_sha256=sha(output) if output.is_file() else None, success=success)
        save()
        if not success:
            print(f'{name}: compile failed or missing fresh output; inspect {logfile}', file=sys.stderr)
            return 1
        print(f'{name}: compiled {output} sha256={entry["output_sha256"]}', flush=True)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
