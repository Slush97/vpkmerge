#!/usr/bin/env python3
"""Export this experiment into the shared aggregation prototype. Local files only.

This is an explicit adapter for the fixed config-comparison protocol, not a
generic uploader or a claim that arbitrary legacy runs meet its requirements.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import sqlite3


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'),
                                    ensure_ascii=False, allow_nan=False).encode()).hexdigest()


def checked_json(db, directory, run_id, relative):
    registered = db.execute('SELECT sha256, bytes FROM artifacts WHERE run_id=? AND path=?',
                            (run_id, relative)).fetchone()
    if registered is None:
        raise ValueError(f'Missing registered evidence: {relative}')
    raw = (directory / relative).read_bytes()
    if (hashlib.sha256(raw).hexdigest(), len(raw)) != tuple(registered):
        raise ValueError(f'Changed registered evidence: {relative}')
    return json.loads(raw)


def export(root):
    report = json.loads((root / 'comparison-results.json').read_text())
    plan = report['plan']
    if plan['protocol_revision'] != 2 or len(report['runs']) != len(plan['order']):
        raise ValueError('Expected the complete revision-2 comparison attempt series')
    db = sqlite3.connect(f'file:{root / "ledger.sqlite3"}?mode=ro', uri=True)
    records = []
    try:
        for run, expected_preset in zip(report['runs'], plan['order']):
            if run['preset'] != expected_preset:
                raise ValueError('Run order differs from the experiment plan')
            row = db.execute('''SELECT r.recorded_utc, r.directory, c.hash, c.payload
                FROM runs r JOIN configurations c ON c.hash=r.config_hash WHERE r.id=?''',
                             (run['run_id'],)).fetchone()
            if row is None or row[0] is None:
                raise ValueError('Run or original recording date missing')
            config = json.loads(row[3])
            if digest(config) != row[2]:
                raise ValueError('Configuration no longer matches its ledger hash')
            directory = Path(row[1]).resolve()
            if not directory.is_relative_to(root):
                raise ValueError('Run is outside the selected experiment')
            treatment = checked_json(db, directory, run['run_id'], 'treatment.json')
            capture = checked_json(db, directory, run['run_id'], 'capture/result.json')
            if treatment['id'] != expected_preset:
                raise ValueError('Treatment differs from the experiment plan')
            hardware = report['hardware']  # reported capture labels, not validated device selection
            settings = config['display']['settings']
            inventory = config['inventory']
            game_build = dict(line.split('=', 1) for line in config['game']['steam_inf'].splitlines() if '=' in line)
            context = {
                'suite': 'nightshift-main-configs',
                'workload_sha256': digest({'scenario': config['scenario_files'], 'map': config['game']['map_sha256']}),
                'protocol_sha256': digest(plan),
                'game_build': f"{game_build['ClientVersion']}:{game_build['SourceRevision']}",
                'runtime': config['runtime']['modlock'], 'runtime_sha256': digest(config['runtime']),
                'launch_policy_sha256': None, 'effective_settings_verified': False,
                'metric_version': '1.1', 'scope': 'estimated_fight',
                'window_policy': 'fight-interior-2-to-28s-filename-midpoint-v1',
                'frame_semantics': 'mangohud-interval-unverified',
                'capture_backend': config['capture']['backend'], 'capture_version': config['capture']['version'],
                'os': inventory['os'], 'os_version': inventory['os_release'],
                'compatibility_layer': config['runtime']['proton'], 'graphics_api': None,
                'driver': hardware['driver'] or None, 'cpu': hardware['cpu'], 'gpu': hardware['gpu'],
                # A coarse bucket of OS-reported usable RAM, not claimed installed DIMM capacity.
                'ram_bucket_gib': math.ceil(inventory['memory_bytes'] / 2**30 / 8) * 8,
                'power_policy': hardware.get('cpuscheduler') or None,
                'cache_policy': 'fresh-prefixes-shared-os-driver',
                'width': int(settings['setting.defaultres']), 'height': int(settings['setting.defaultresheight']),
                'render_scale': float(settings['setting.mat_viewportscale']),
                'upscaler': settings['setting.r_citadel_upscaling'],
                'frame_cap': float(settings['setting.fps_max']), 'vsync': settings['setting.mat_vsync'] != '0',
                'preset_id': treatment['id'], 'preset_version': treatment['version'],
                'settings_sha256': digest({'gameinfo': treatment['gameinfo_sha256'], 'video': treatment['video_sha256']}),
            }
            success = capture.get('protocol_checks_passed') is True and capture.get('fight_metrics') is not None
            if success != run['protocol_checks_passed']:
                raise ValueError('Report and registered capture disagree on protocol completion')
            windows = []
            if success:
                if capture['fight_metrics'] != run['diagnostic_metrics'] or capture['metric_version'] != '1.1':
                    raise ValueError('Report and registered capture disagree on metrics')
                metrics = capture['fight_metrics']
                windows.append({'index': 1, **{key: metrics[key] for key in (
                    'duration_seconds', 'frame_count', 'average_fps', 'one_percent_low_fps',
                    'p95_frame_time_ms', 'p99_frame_time_ms', 'maximum_frame_time_ms')},
                    'frames_over_50ms': capture['fight_window']['hitch_counts']['50']})
            records.append({
                'contributor_id': 'local-pilot', 'assessment': 'provisional',
                'assessment_version': 'local-pilot-review-1',
                'reasons': ['render-clock-alignment-unverified', 'reconstruction-unverified',
                            'effective-settings-unverified', 'complete-launch-policy-unavailable'],
                'submission': {'schema_version': 1, 'run_id': run['run_id'],
                    'experiment_id': digest(plan), 'machine_id': 'local-machine-1',
                    'recorded_at': row[0], 'context': context,
                    'outcome': 'completed' if success else 'failed',
                    'source_sha256': capture.get('raw_sha256'), 'windows': windows},
            })
    finally:
        db.close()
    return records


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('experiment', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    records = export(args.experiment.resolve())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open('x') as stream:
        json.dump(records, stream, indent=2, allow_nan=False)
        stream.write('\n')
    print(f'Exported {len(records)} provisional attempts; no network requests')
