#!/usr/bin/env python3
"""Convert the observed MangoHud 0.8.4 CSV layout to per-frame-csv.

This is a format conversion, not a capture-validity or frame-semantics check.
"""
import argparse
import csv
import hashlib
import io
import json
import math
from pathlib import Path


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('source', type=Path)
    ap.add_argument('output', type=Path, help='new CSV path; existing files are never replaced')
    args = ap.parse_args()
    raw = args.source.read_bytes()
    stream = io.StringIO(raw.decode('utf-8-sig'), newline='')
    reader = csv.reader(stream)
    if next(reader, [])[:3] != ['os', 'cpu', 'gpu'] or not next(reader, []):
        raise ValueError('expected MangoHud system metadata and values')
    fields = next(reader, [])
    if 'elapsed' not in fields or 'frametime' not in fields:
        raise ValueError('expected elapsed and frametime columns')
    time_index, frame_index = fields.index('elapsed'), fields.index('frametime')
    converted = []
    for line, row in enumerate(reader, 4):
        try:
            timestamp = int(row[time_index]) / 1_000_000_000
            duration = float(row[frame_index])
        except (ValueError, IndexError) as e:
            raise ValueError(f'invalid frame at row {line}') from e
        if not math.isfinite(timestamp) or not math.isfinite(duration) or timestamp < 0 or duration <= 0:
            raise ValueError(f'invalid frame at row {line}')
        if converted and (timestamp <= converted[-1][0] or abs((timestamp - converted[-1][0]) * 1000 - duration) > 1):
            raise ValueError(f'non-increasing or inconsistent frame interval at row {line}')
        converted.append((timestamp, duration))
    if not converted:
        raise ValueError('no frames')
    with args.output.open('x', newline='', encoding='utf-8') as f:
        writer = csv.writer(f)
        writer.writerow(['timestamp_seconds', 'frame_time_ms'])
        writer.writerows(converted)
    print(json.dumps({'source': str(args.source), 'source_sha256': hashlib.sha256(raw).hexdigest(),
                      'output': str(args.output), 'frames': len(converted),
                      'semantics': 'unverified', 'valid_benchmark': 'not assessed'}))


if __name__ == '__main__':
    main()
