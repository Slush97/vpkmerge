#!/usr/bin/env python3
"""Execute a fixed launch order, retaining failed attempts without scoring them."""
import hashlib
import json
from pathlib import Path
import shutil
import signal
import subprocess
import sys

root = Path(sys.argv[1]).resolve()
plan = json.loads((root / 'comparison-plan.json').read_text())
harness = Path(__file__).with_name('run-config-trial.py')
if hashlib.sha256(harness.read_bytes()).hexdigest() != plan['harness_sha256']:
    raise SystemExit('Harness differs from the predeclared comparison plan')
summary = root / 'comparison-runs.json'
runs = json.loads(summary.read_text()) if summary.exists() else []
if [r['preset'] for r in runs] != plan['order'][:len(runs)]:
    raise SystemExit('Existing attempts do not match the predeclared order')
child = None


def cancel(signum, frame):
    if child is not None and child.poll() is None:
        child.send_signal(signal.SIGTERM)
        child.wait(timeout=30)
    raise SystemExit(130)


signal.signal(signal.SIGTERM, cancel)
signal.signal(signal.SIGINT, cancel)
for index, preset in enumerate(plan['order'], 1):
    if index <= len(runs):
        continue
    if shutil.disk_usage(root).free < 1.5 * 1024**3:
        raise SystemExit('Insufficient space for another isolated launch; evidence retained')
    before = set(root.glob('session-*'))
    print(f'COMPARISON_TRIAL {index}/{len(plan["order"])} {preset}', flush=True)
    with (root / f'trial-{index:02d}-launcher.log').open('w') as log:
        child = subprocess.Popen([sys.executable, str(harness), str(root), preset, f'trial-{index:02d}'],
                                 stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        for line in child.stdout:
            log.write(line); log.flush()
            print(line, end='', flush=True)
        code = child.wait()
    created = set(root.glob('session-*')) - before
    if len(created) != 1:
        raise SystemExit('Expected exactly one new run directory')
    run = created.pop()
    capture_path = run / 'capture/result.json'
    capture = json.loads(capture_path.read_text()) if capture_path.exists() else {}
    ledger = json.loads((run / 'run-ledger.json').read_text())
    accepted = code == 0 and capture.get('status') == 'captured' and capture.get('protocol_checks_passed') is True
    entry = {'index': index, 'preset': preset, 'directory': str(run), 'run_id': ledger['id'],
             'launcher_exit_code': code, 'protocol_checks_passed': accepted,
             'diagnostic_metrics': capture.get('fight_metrics')}
    runs.append(entry)
    summary.write_text(json.dumps(runs, indent=2) + '\n')
    print('COMPARISON_RESULT ' + json.dumps(entry), flush=True)
    if not accepted:
        print('Trial failed protocol checks; retained as a failed attempt, without an FPS score', flush=True)
print('COMPARISON_COMPLETE', flush=True)
