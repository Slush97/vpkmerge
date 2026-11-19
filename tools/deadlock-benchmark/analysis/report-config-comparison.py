#!/usr/bin/env python3
"""Summarize independent config trials without promoting them to valid scores."""
import csv
from datetime import datetime, timezone
import html
import json
from pathlib import Path
import re
import statistics
import sys
from urllib.parse import quote

root = Path(sys.argv[1]).resolve()
plan = json.loads((root / 'comparison-plan.json').read_text())
runs = json.loads((root / 'comparison-runs.json').read_text())
if len(runs) != len(plan['order']):
    raise SystemExit('Comparison attempt series is incomplete')
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'runtime'))
from benchmark_ledger import Ledger

ledger = Ledger(root / 'ledger.sqlite3')
hardware = None
common_configuration = None
arm_displays = {}
for run in runs:
    verification = ledger.verify(run['run_id'])
    if not verification['integrity_ok']:
        raise SystemExit(str(verification))
    run['artifact_integrity_ok'] = True
    if not run['protocol_checks_passed']:
        run['failure'] = ledger.export(run['run_id'])['result'].get('error')
        continue
    p = Path(run['directory'])
    configuration = ledger.export(run['run_id'])['configuration']
    treatment = json.loads((p / 'treatment.json').read_text())
    common = {'scenario_files': configuration['scenario_files'],
              'steam_inf': configuration['game']['steam_inf'],
              'map_sha256': configuration['game']['map_sha256'],
              'runtime': configuration['runtime'], 'server': configuration['server']}
    if common_configuration is not None and common != common_configuration:
        raise SystemExit('Successful runs differ in workload, game build, map, or runtime')
    common_configuration = common
    if configuration['game']['gameinfo_sha256'] != treatment['gameinfo_sha256']:
        raise SystemExit('Recorded gameinfo hash differs from prepared treatment')
    display = configuration['display']
    if run['preset'] in arm_displays and arm_displays[run['preset']] != display:
        raise SystemExit('Input display configuration changed within a preset')
    arm_displays[run['preset']] = display
    result = json.loads((p / 'capture/result.json').read_text())
    hardware = hardware or result.get('raw_system_metadata')
    run['fight_window'] = result['fight_window']
    run['capture_sha256'] = result['raw_sha256']
    run['configured_display'] = json.loads((p / 'display.json').read_text())
    log = (p / 'server-launch.log').read_text(errors='replace')
    run['pass_end'] = [s.strip() for s in log.splitlines() if 'REPLAY PROBE BENCH END' in s]
    run['update_summary'] = [s.strip() for s in log.splitlines() if 'update_summary pass=measured' in s]
    if len(run['pass_end']) != 1 or 'max_tick_skew=0' not in run['pass_end'][0]:
        raise SystemExit('Missing successful, synchronized measured pass')
    after = dict(re.findall(r'"setting\.([^"\s]+)"\s*"([^"\r\n]*)"', (p / 'video-after.txt').read_text()))
    for key, expected in {'defaultres': 2560, 'defaultresheight': 1440,
                          'mat_viewportscale': 1, 'r_citadel_upscaling': 0, 'mat_vsync': 0}.items():
        if float(after[key]) != expected:
            raise SystemExit(f'{run["run_id"]}: video setting changed: {key}')
    run['artifact_integrity_ok'] = True
ledger.close()

names = {'sqooky-default': 'Sqooky Default', 'optilock-fps': 'Dacooder OptiLock FPS'}
summary = {}
fields = ['average_fps', 'one_percent_low_fps', 'p99_frame_time_ms', 'maximum_frame_time_ms']
for preset, name in names.items():
    attempts = [r for r in runs if r['preset'] == preset]
    subset = [r for r in attempts if r['protocol_checks_passed']]
    treatment = json.loads((root / 'variants' / preset / 'preset.json').read_text())
    aggregate = {}
    for field in fields:
        if not subset:
            aggregate[field] = None
            continue
        values = [r['diagnostic_metrics'][field] for r in subset]
        # Sensitivity is not a statistical confidence interval.
        sensitivity = [[r['fight_window']['sensitivity_metrics'][str(shift)][field]
                        for shift in [-.5, 0, .5]] for r in subset]
        aggregate[field] = {'median': statistics.median(values), 'min': min(values), 'max': max(values),
                            'timestamp_sensitivity_median_range': [statistics.median(min(v) for v in sensitivity),
                                                                   statistics.median(max(v) for v in sensitivity)]}
    summary[preset] = {'name': name, 'independent_launches': len(subset), 'attempts': len(attempts),
                       'failed_attempts': len(attempts)-len(subset),
                       'version': treatment['version'], 'upstream': treatment['upstream'],
                       'gameinfo_sha256': treatment['gameinfo_sha256'], 'video_sha256': treatment['video_sha256'],
                       'metrics': aggregate,
                       'total_observation_seconds': sum(r['diagnostic_metrics']['duration_seconds'] for r in subset),
                       'hitch_counts': {key: sum(r['fight_window']['hitch_counts'][key] for r in subset)
                                        for key in (subset[0]['fight_window']['hitch_counts'] if subset else [])}}
deltas = {field: (summary['optilock-fps']['metrics'][field]['median'] /
                   summary['sqooky-default']['metrics'][field]['median'] - 1) * 100
          if all(summary[p]['metrics'][field] is not None for p in names) else None
          for field in ['average_fps', 'one_percent_low_fps']}
result = {'schema_version': 1, 'created_utc': datetime.now(timezone.utc).isoformat(),
          'plan': plan, 'hardware': hardware, 'summary': summary, 'optilock_vs_sqooky_percent': deltas, 'runs': runs,
          'comparison_eligible': False, 'measurement_valid': False,
          'limitations': [
              'Exploratory Modlock reconstruction, not original replay or live match performance.',
              'Approximate phase mapping from server wall marker and MangoHud filename second; sensitivity is not a guaranteed error bound.',
              'Same display/render scale/cap/aspect controls, but author visual-quality and camera settings otherwise differ.',
              'Sqooky supplies no sibling video.txt in the selected preset; baseline video options remain. OptiLock supplies its own video options. This is not an equal-image-quality comparison.',
              'Grimoire filters applied, including known incompatible settings; author VPK bundles were not installed.',
              'Four fixed attempts per preset; failures retained without FPS scores. Performance summaries are conditional on successful capture.',
              'Both presets have had client.dll+0x17349c6 access violations across attempts; cause is unresolved and may involve the shared harness.',
              'One Linux/Proton machine; sampled foreground checks, shared OS/driver caches.',
              'Synthetic gunfire and inferred casts; requested/rejected/observed counts remain available per run.',
          ]}
(root / 'comparison-results.json').write_text(json.dumps(result, indent=2) + '\n')

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

colors = {'sqooky-default': '#4477AA', 'optilock-fps': '#CC6677'}
fig, axes = plt.subplots(1, 2, figsize=(11, 4.5), constrained_layout=True)
for ax, field, title in zip(axes, ['average_fps', 'one_percent_low_fps'], ['Average FPS', '1% low FPS']):
    for r in runs:
        if not r['protocol_checks_passed']:
            ax.axvline(r['index'], color='#888888', linestyle=':', alpha=.4)
            ax.text(r['index'], .02, 'failed', transform=ax.get_xaxis_transform(),
                    ha='center', color='#666666', fontsize=8)
    for preset, name in names.items():
        selected = [r for r in runs if r['preset'] == preset and r['protocol_checks_passed']]
        ax.plot([r['index'] for r in selected], [r['diagnostic_metrics'][field] for r in selected],
                'o-', color=colors[preset], label=name, linewidth=1.3)
        for r in selected:
            ax.annotate(f'{r["diagnostic_metrics"][field]:.0f}',
                        (r['index'], r['diagnostic_metrics'][field]), xytext=(0, 7),
                        textcoords='offset points', ha='center', fontsize=8)
    ax.set_title(title); ax.set_xlabel('Independent launch order'); ax.set_ylabel('FPS, higher is better')
    ax.set_xticks(range(1, len(runs)+1)); ax.grid(alpha=.2)
    peak = max((r['diagnostic_metrics'][field] for r in runs if r['protocol_checks_passed']), default=1)
    ax.set_ylim(0, peak * 1.16); ax.legend(fontsize=8)
fig.suptitle('Sqooky vs Dacooder: exploratory 26-second fight windows\n2560×1440, 100% render scale, uncapped; approximate phase mapping', fontsize=12)
fig.savefig(root / 'comparison-chart.png', dpi=160)
plt.close(fig)

rows = []
def metric_cell(metric):
    if metric is None:
        return 'No valid capture'
    return f'{metric["median"]:.1f} ({metric["min"]:.1f}–{metric["max"]:.1f})'

for preset, value in summary.items():
    m = value['metrics']
    p99 = f'{m["p99_frame_time_ms"]["median"]:.2f} ms' if m['p99_frame_time_ms'] else 'Unavailable'
    source = value['upstream']
    url = f'https://github.com/{source["repo"]}/blob/{source["commit"]}/{quote(source["path"])}'
    rows.append(f'<tr><td><a href="{html.escape(url)}">{html.escape(value["name"])}</a></td><td>{value["independent_launches"]}/{value["attempts"]}</td>'
                f'<td>{metric_cell(m["average_fps"])}</td>'
                f'<td>{metric_cell(m["one_percent_low_fps"])}</td><td>{p99}</td></tr>')
details = []
for r in runs:
    relative = Path(r['directory']).relative_to(root).as_posix()
    outcome = 'capture passed checks' if r['protocol_checks_passed'] else 'FAILED, no FPS sample'
    window_link = (f', <a href="{relative}/capture/fight-window.json">window and timing sensitivity</a>'
                   if r['protocol_checks_passed'] else '')
    details.append(f'<li>Launch {r["index"]}: {html.escape(names[r["preset"]])}, {outcome}, '
                   f'<a href="{relative}/run-ledger.json">ledger</a>, '
                   f'<a href="{relative}/trial-observation.json">observations</a>{window_link}</li>')
doc = f'''<!doctype html><meta charset="utf-8"><title>Sqooky vs Dacooder benchmark</title>
<style>body{{font:16px system-ui;max-width:1100px;margin:40px auto;padding:0 24px;line-height:1.5}}table{{border-collapse:collapse;width:100%}}th,td{{padding:12px;border-bottom:1px solid #ccc;text-align:left}}img{{max-width:100%}}small{{color:#555}}</style>
<h1>Sqooky vs Dacooder on this machine</h1><p>Exploratory comparison. Values below are medians of independent launches; parentheses show the run range. Loading, warm-up and resets are excluded from the estimated fight windows.</p>
<p>{html.escape((hardware or {}).get('cpu', 'CPU unavailable'))} · {html.escape((hardware or {}).get('gpu', 'GPU unavailable'))} · Linux/Proton · 2560×1440 · 100% render scale · uncapped.</p>
<table><tr><th>Config</th><th>Completed/attempted</th><th>Average FPS</th><th>1% low FPS</th><th>p99 frame time</th></tr>{''.join(rows)}</table>
<p>OptiLock relative changes in percent: {html.escape(json.dumps(deltas))}. These are conditional observed differences, not statistical significance claims. Failed launches are retained in the evidence below.</p>
<img src="comparison-chart.png" alt="FPS and 1% low for each independent launch">
<h2>Scope and limitations</h2><ul>{''.join('<li>'+html.escape(x)+'</li>' for x in result['limitations'])}</ul>
<h2>Evidence</h2><p><a href="comparison-results.json">Complete results, version pins, settings hashes and timing sensitivity</a> · <a href="comparison-plan.json">Predeclared order and controls</a></p><ol>{''.join(details)}</ol>
<p><a href="comparison-runs-aborted-v1.json">Earlier aborted series</a> · <a href="crash-evidence/index.json">Crash exception records</a>. One OptiLock attempt in the earlier series and one Sqooky attempt in the final series crashed at the same client-module offset. The completion counts above describe the final series only.</p>
<small>Generated {html.escape(result['created_utc'])}. Raw streams and configurations are retained in the local ledger.</small>'''
(root / 'comparison-report.html').write_text(doc)
print(json.dumps({'summary': summary, 'optilock_vs_sqooky_percent': deltas}, indent=2))
