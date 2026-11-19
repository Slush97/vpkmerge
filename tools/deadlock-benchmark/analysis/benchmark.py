#!/usr/bin/env python3
"""Analyze explicitly aligned per-frame captures; see README.md for the contract."""
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path
import statistics
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'runtime'))
from benchmark_metrics import METRIC_VERSION, metrics

VERSION = '1.0'
CHECKS = ('warmup_completed', 'supported_build', 'complete_interval', 'effects_verified',
          'no_load_screens', 'no_unexpected_pause', 'no_lost_capture_data', 'target_process_verified')
META = ('planned_duration_seconds', 'benchmark_version', 'map_hash', 'scenario_version', 'scenario_hash', 'game_build',
        'capture_tool', 'capture_tool_version', 'frame_semantics', 'frame_generation', 'server_setup',
        'settings', 'hardware', 'os', 'gpu_driver', 'mods', 'shader_cache_policy')


def number(value, name):
    if isinstance(value, bool):
        raise ValueError(f'{name} must be a finite number')
    try:
        result = float(value)
    except (ValueError, TypeError):
        raise ValueError(f'{name} must be a finite number') from None
    if not math.isfinite(result):
        raise ValueError(f'{name} must be a finite number')
    return result


def read_frames(path, adapter):
    if adapter == 'presentmon':
        time_col, frame_col = 'TimeInSeconds', 'MsBetweenPresents'
    elif adapter == 'per-frame-csv':
        time_col, frame_col = 'timestamp_seconds', 'frame_time_ms'
    else:
        raise ValueError('adapter must be presentmon or per-frame-csv')
    frames = []
    with path.open(newline='', encoding='utf-8-sig') as source:
        reader = csv.DictReader(source)
        if not reader.fieldnames or not {time_col, frame_col} <= set(reader.fieldnames):
            raise ValueError(f'CSV requires columns {time_col} and {frame_col}')
        for line, row in enumerate(reader, 2):
            timestamp = number(row[time_col], f'row {line} timestamp')
            frame_time = number(row[frame_col], f'row {line} frame time')
            if frame_time == 0 and not frames and adapter == 'presentmon':
                frames.append((timestamp, None))
                continue
            if frame_time <= 0:
                raise ValueError(f'row {line} frame time must be positive')
            if frames and timestamp <= frames[-1][0]:
                raise ValueError(f'row {line} timestamps must strictly increase; isolate one frame stream')
            if frames and abs((timestamp - frames[-1][0]) - frame_time / 1000) > 0.001:
                raise ValueError(f'row {line} frame time does not match timestamp interval (1 ms tolerance)')
            frames.append((timestamp, frame_time))
    if not frames:
        raise ValueError('capture has no frame rows')
    return frames


def graph(frames, start, end, path):
    width, height, margin = 960, 340, 50
    maximum = max(t for _, t in frames)
    points = ' '.join(f'{margin+(x-start)/(end-start)*(width-2*margin):.2f},{height-margin-y/maximum*(height-2*margin):.2f}' for x, y in frames)
    svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}"><rect width="100%" height="100%" fill="white"/><path d="M {margin} {margin} V {height-margin} H {width-margin}" fill="none" stroke="black"/><polyline points="{points}" fill="none" stroke="#135db1" stroke-width="1"/><text x="{margin}" y="22" font-family="sans-serif" font-size="14">Frame time (ms); maximum {maximum:.3f}</text><text x="{margin}" y="{height-12}" font-family="sans-serif" font-size="14">0 s</text><text x="{width-margin-85}" y="{height-12}" font-family="sans-serif" font-size="14">{end-start:.3f} s</text></svg>'''
    path.write_text(svg, encoding='utf-8')


def analyze(manifest_path, output):
    manifest_path = manifest_path.resolve()
    manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
    if not isinstance(manifest, dict):
        raise ValueError('manifest must be a JSON object')
    result = {'schema_version': VERSION, 'metric_version': METRIC_VERSION, 'adapter_version': VERSION,
              'manifest': manifest, 'manifest_path': str(manifest_path), 'valid': False, 'invalid_reasons': []}
    reasons = result['invalid_reasons']
    metadata = manifest.get('metadata', {})
    if not isinstance(metadata, dict):
        metadata = {}
    for key in META:
        if key not in metadata or metadata[key] in (None, '', 'unknown', {}) or (metadata[key] == [] and key != 'mods'):
            reasons.append(f'missing or unknown metadata: {key}')
    try:
        if number(metadata.get('planned_duration_seconds'), 'planned duration') <= 0:
            reasons.append('planned duration must be positive')
    except ValueError as exc:
        reasons.append(str(exc))
    checks = manifest.get('checks', {})
    if not isinstance(checks, dict):
        checks = {}
    for check in CHECKS:
        if checks.get(check) is not True:
            reasons.append(f'check not confirmed: {check}')
    supplied_reasons = manifest.get('invalid_reasons', [])
    if not isinstance(supplied_reasons, list):
        raise ValueError('invalid_reasons must be a list')
    for reason in supplied_reasons:
        reasons.append(str(reason))
    try:
        raw = (manifest_path.parent / manifest['raw_capture']).resolve()
        result['raw_capture_path'] = str(raw)
        result['raw_capture_sha256'] = hashlib.sha256(raw.read_bytes()).hexdigest()
        adapter = manifest['adapter']
        if adapter == 'presentmon' and metadata.get('frame_semantics') != 'presented':
            raise ValueError('presentmon adapter requires frame_semantics presented')
        markers = manifest['markers']
        start = number(markers['capture_start_seconds'], 'capture start marker')
        end = number(markers['capture_end_seconds'], 'capture end marker')
        scenario_start = number(markers['scenario_start_seconds'], 'scenario start marker')
        scenario_end = number(markers['scenario_end_seconds'], 'scenario end marker')
        if not markers.get('alignment_method') or markers['alignment_method'] == 'unknown':
            raise ValueError('marker alignment_method is required')
        if end <= start or scenario_end <= scenario_start:
            raise ValueError('end marker must follow start marker in each clock')
        result['observed_durations_seconds'] = {'capture': end - start, 'scenario': scenario_end - scenario_start}
        frames = read_frames(raw, adapter)
        if frames[0][0] > start or frames[-1][0] < end:
            raise ValueError('capture does not cover both markers')
        selected = [frame for frame in frames if start < frame[0] <= end]
        if any(frame[1] is None for frame in selected):
            raise ValueError('selected interval includes an undefined first frame time')
        if not selected:
            raise ValueError('no frame events within marker interval')
        if not reasons:
            graph(selected, start, end, output / 'frame-times.svg')
            result['metrics'] = metrics(selected, end - start)
            result['graph'] = 'frame-times.svg'
            result['valid'] = True
    except (OSError, KeyError, TypeError, ValueError, csv.Error) as exc:
        reasons.append(f'capture invalid: {exc}')
    (output / 'run.json').write_text(json.dumps(result, indent=2, allow_nan=False) + '\n', encoding='utf-8')
    return result


def report(paths):
    runs = [json.loads(Path(path).read_text(encoding='utf-8')) for path in paths]
    if len(runs) < 3 or any(not run.get('valid') for run in runs):
        raise ValueError('report requires at least three valid runs')
    first = runs[0]
    if any(run.get('metric_version') != first.get('metric_version') or
           run.get('adapter_version') != first.get('adapter_version') or
           run['manifest'].get('adapter') != first['manifest'].get('adapter') or
           run['manifest']['metadata'] != first['manifest']['metadata'] for run in runs[1:]):
        raise ValueError('runs differ in metadata, adapter or metric version; do not combine them')
    if len({run['raw_capture_sha256'] for run in runs}) != len(runs):
        raise ValueError('repeated raw captures are not independent runs')
    summary = {}
    for key in ('average_fps', 'one_percent_low_fps', 'p95_frame_time_ms', 'p99_frame_time_ms'):
        if key == 'p95_frame_time_ms' and first.get('metric_version') == '1.0':
            continue  # Historical 1.0 reports did not record p95.
        values = [run['metrics'][key] for run in runs]
        summary[key] = {'runs': values, 'median': statistics.median(values), 'min': min(values), 'max': max(values)}
    return {'schema_version': VERSION, 'metric_version': first['metric_version'], 'run_files': [str(Path(p).resolve()) for p in paths],
            'metadata': first['manifest']['metadata'],
            'observed_durations_seconds': [run['observed_durations_seconds'] for run in runs], 'metrics': summary}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    run = sub.add_parser('run', help='validate and analyze a capture')
    run.add_argument('manifest')
    run.add_argument('--output', required=True, help='new output directory')
    rep = sub.add_parser('report', help='summarize at least three comparable valid runs')
    rep.add_argument('runs', nargs='+')
    rep.add_argument('--output', required=True, help='new JSON report file')
    args = parser.parse_args()
    try:
        if args.command == 'run':
            output = Path(args.output)
            output.mkdir(parents=True, exist_ok=False)
            result = analyze(Path(args.manifest), output)
            print(json.dumps({'valid': result['valid'], 'invalid_reasons': result['invalid_reasons'], 'result': str(output / 'run.json')}))
            return 0 if result['valid'] else 2
        result = report(args.runs)
        with Path(args.output).open('x', encoding='utf-8') as dest:
            json.dump(result, dest, indent=2, allow_nan=False)
            dest.write('\n')
        return 0
    except (OSError, ValueError, KeyError, TypeError, json.JSONDecodeError) as exc:
        print(f'error: {exc}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
