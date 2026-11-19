import sys
import csv
import hashlib
import json
from pathlib import Path
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

if len(sys.argv) != 2:
    raise SystemExit('Usage: python '+Path(__file__).name+' /path/to/comparison-experiment')
root = Path(sys.argv[1]).resolve()
(root / 'graphs').mkdir(exist_ok=True)
out = root / 'graphs'
data = json.loads((root / 'comparison-results.json').read_text())
ids = ['sqooky-default', 'optilock-fps']
names = ['Sqooky Default\n3 completed / 4 attempts', 'Dacooder OptiLock FPS\n4 completed / 4 attempts']
colors = ['#3974AC', '#CC6075']
plt.rcParams.update({'font.family': 'DejaVu Sans', 'font.size': 11, 'axes.spines.top': False,
                     'axes.spines.right': False, 'figure.facecolor': 'white'})

fig, axes = plt.subplots(1, 2, figsize=(12, 5.7))
for ax, metric, title in zip(axes, ['average_fps', 'one_percent_low_fps'],
                            ['Average FPS', '1% low FPS']):
    for i, (preset, color) in enumerate(zip(ids, colors)):
        m = data['summary'][preset]['metrics'][metric]
        values = [r['diagnostic_metrics'][metric] for r in data['runs']
                  if r['preset'] == preset and r['protocol_checks_passed']]
        ax.bar(i, m['median'], width=.57, color=color, alpha=.22)
        ax.errorbar(i, m['median'], yerr=[[m['median'] - m['min']], [m['max'] - m['median']]],
                    fmt='none', ecolor=color, capsize=10, linewidth=2)
        ax.scatter(i + np.linspace(-.12, .12, len(values)), values, s=38, color=color, zorder=4)
        ax.text(i, m['max'] + (12 if metric == 'average_fps' else 8),
                f"{m['median']:.1f}", ha='center', weight='bold', fontsize=17, color=color)
    ax.set_title(title, pad=16, weight='bold')
    ax.set_xticks([0, 1], names, fontsize=10)
    ax.set_ylabel('FPS · higher is better')
    ax.set_ylim(0, 355 if metric == 'average_fps' else 200)
    ax.set_axisbelow(True); ax.grid(axis='y', alpha=.17)
fig.suptitle('Sqooky vs Dacooder: almost identical average FPS', fontsize=17, weight='bold', y=.97)
fig.text(.5, .89, '1440p · 100% render scale · uncapped · estimated 26-second fight windows', ha='center', color='#555555')
fig.text(.5, .08, 'Bars = medians   •   Dots = completed runs   •   Whiskers = observed min–max, not confidence intervals', ha='center', fontsize=10)
fig.text(.5, .035, 'Provisional config-only comparison: timing is approximate; failed attempts have no FPS score.', ha='center', fontsize=10, color='#666666')
fig.subplots_adjust(top=.78, bottom=.21, left=.08, right=.98, wspace=.29)
for ext in ['png', 'svg']:
    fig.savefig(out / f'fps-comparison.{ext}', dpi=170)
plt.close(fig)

fig, axes = plt.subplots(1, 2, figsize=(12, 5.7), sharey=True)
for ax, preset, name, color in zip(axes, ids, names, colors):
    selected = [r for r in data['runs'] if r['preset'] == preset and r['protocol_checks_passed']]
    for j, run in enumerate(selected):
        start = run['fight_window']['capture_seconds'][0]
        buckets = np.full(260, np.nan)
        with (Path(run['directory']) / 'capture/fight-window-frames.csv').open() as f:
            for row in csv.DictReader(f):
                t, ms = float(row['timestamp_seconds']) - start, float(row['frame_time_ms'])
                index = min(259, max(0, int(t / .1)))
                buckets[index] = ms if np.isnan(buckets[index]) else max(buckets[index], ms)
        ax.plot(np.arange(260)*.1 + .05, buckets, color=color,
                alpha=.3 + j*.17, linewidth=.85, label=f"Run {run['index']}")
    ax.axhline(16.6667, color='#888888', linestyle='--', linewidth=.8)
    ax.text(.5, 17.05, '16.7 ms ≈ 60 FPS', color='#777777', fontsize=9)
    ax.set_title(name.split('\n')[0], color=color, weight='bold', pad=12)
    ax.set_xlim(0, 26); ax.set_ylim(0, 18.2)
    ax.set_xlabel('Seconds into estimated fight window')
    ax.grid(alpha=.15); ax.legend(loc='upper right', fontsize=8, ncol=2, framealpha=.95)
axes[0].set_ylabel('Worst frame time per 0.1-second bin (ms) · lower is better')
fig.suptitle('Frame-time spikes across all completed runs', fontsize=17, weight='bold', y=.97)
fig.text(.5, .89, 'Each line is one run; bin maxima retain every run’s largest spike.', ha='center', color='#555555')
fig.text(.5, .07, 'No frame exceeded 16.7 ms in the accepted nominal windows. Loading, warm-up and resets are excluded.', ha='center', fontsize=10)
fig.text(.5, .03, 'Approximate alignment: small horizontal offsets can reflect timing uncertainty. This is not a validated match benchmark.', ha='center', fontsize=9, color='#666666')
fig.subplots_adjust(top=.79, bottom=.21, left=.09, right=.98, wspace=.13)
for ext in ['png', 'svg']:
    fig.savefig(out / f'frame-time-spikes.{ext}', dpi=170)
plt.close(fig)
files = {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
         for p in sorted(out.iterdir()) if p.suffix in ['.png', '.svg']}
(out / 'manifest.json').write_text(json.dumps({'source_results_sha256': hashlib.sha256((root / 'comparison-results.json').read_bytes()).hexdigest(),
                                            'files': files}, indent=2) + '\n')
print(json.dumps(files, indent=2))
