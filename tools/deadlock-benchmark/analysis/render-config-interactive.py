"""Offline, interactive view of the retained config-comparison results."""
import csv
import hashlib
import html
import json
from pathlib import Path
import sys

if len(sys.argv) != 2:
    raise SystemExit('Usage: python '+Path(__file__).name+' /path/to/comparison-experiment')
root = Path(sys.argv[1]).resolve()
(root / 'graphs').mkdir(exist_ok=True)
sys.path.insert(0, str(root / 'graphs/.plotly-deps'))
import plotly
import plotly.graph_objects as go
from plotly.subplots import make_subplots

results = json.loads((root / 'comparison-results.json').read_text())
presets = ['sqooky-default', 'optilock-fps']
labels = {'sqooky-default': 'Sqooky Default', 'optilock-fps': 'Dacooder OptiLock FPS'}
colors = {'sqooky-default': '#3974AC', 'optilock-fps': '#CC6075'}
runs = results['runs']
valid = [r for r in runs if r['protocol_checks_passed']]
config = {'responsive': True, 'displaylogo': False,
          'toImageButtonOptions': {'format': 'png', 'scale': 2}}
summary = make_subplots(rows=1, cols=2, subplot_titles=['Average FPS', '1% low FPS'], horizontal_spacing=.16)
for col, metric in enumerate(['average_fps', 'one_percent_low_fps'], 1):
    for i, preset in enumerate(presets):
        stats = results['summary'][preset]['metrics'][metric]
        group = [r for r in valid if r['preset'] == preset]
        summary.add_trace(go.Bar(x=[i], y=[stats['median']], width=.52,
            marker_color=colors[preset], opacity=.25, showlegend=False,
            hovertemplate=f'{labels[preset]}<br>Median: %{{y:.1f}} FPS<extra></extra>'), row=1, col=col)
        summary.add_trace(go.Scatter(x=[i], y=[stats['median']], mode='markers+text',
            marker={'symbol': 'line-ew', 'size': 30, 'color': colors[preset], 'line': {'width': 3}},
            text=[f"{stats['median']:.1f}"], textposition='top center',
            textfont={'size': 20, 'color': colors[preset]}, showlegend=False,
            error_y={'type': 'data', 'symmetric': False,
                     'array': [stats['max']-stats['median']], 'arrayminus': [stats['median']-stats['min']],
                     'color': colors[preset], 'width': 10},
            hovertemplate=f"Median {stats['median']:.1f} FPS<br>Observed range {stats['min']:.1f}–{stats['max']:.1f}<br>Not a confidence interval<extra></extra>"), row=1, col=col)
        n = len(group)
        summary.add_trace(go.Scatter(x=[i-.12+j*.24/max(1,n-1) for j in range(n)],
            y=[r['diagnostic_metrics'][metric] for r in group], mode='markers',
            marker={'size': 10, 'color': colors[preset]}, customdata=[r['index'] for r in group],
            showlegend=False, hovertemplate=labels[preset]+'<br>Attempt %{customdata}<br>%{y:.2f} FPS<extra></extra>'), row=1, col=col)
    summary.update_xaxes(tickmode='array', tickvals=[0,1], ticktext=['Sqooky', 'Dacooder'], range=[-.5,1.5], row=1,col=col)
    summary.update_yaxes(title_text='FPS · higher is better', range=[0,355 if col==1 else 200], row=1,col=col)
summary.update_layout(template='plotly_white', height=430, margin={'l':65,'r':25,'t':55,'b':45}, font={'family':'system-ui, sans-serif'})

frames = go.Figure()
for r in valid:
    start = r['fight_window']['capture_seconds'][0]
    bins = {}
    with (Path(r['directory']) / 'capture/fight-window-frames.csv').open() as f:
        for row in csv.DictReader(f):
            t, ms = float(row['timestamp_seconds'])-start, float(row['frame_time_ms'])
            index = max(0,min(259,int(t/.1)))
            if index not in bins or ms > bins[index][1]:
                bins[index] = (t, ms)
    samples = sorted(bins.items())
    frames.add_trace(go.Scatter(x=[i*.1+.05 for i,_ in samples], y=[v[1] for _,v in samples],
        customdata=[v[0] for _,v in samples], mode='lines',
        name=f"{'Sqooky' if r['preset']=='sqooky-default' else 'Dacooder'} · attempt {r['index']}",
        line={'color':colors[r['preset']], 'width':1.3}, opacity=.65,
        hovertemplate='Bin midpoint: %{x:.2f}s<br>Worst frame: %{y:.3f} ms<br>Frame timestamp: %{customdata:.3f}s<extra>%{fullData.name}</extra>'))
frames.add_hline(y=16.6667, line_dash='dash', line_color='#999999',
                 annotation_text='16.7 ms ≈ 60 FPS', annotation_position='top left')
frames.update_layout(template='plotly_white', height=480, hovermode='closest',
    margin={'l':65,'r':25,'t':35,'b':35}, font={'family':'system-ui, sans-serif'},
    legend={'orientation':'h','y':1.2,'x':0},
    xaxis={'title':'Seconds into estimated fight window','range':[0,26], 'rangeslider':{'visible':True,'thickness':.12}},
    yaxis={'title':'Worst frame in each 0.1-second bin (ms)','range':[0,18.2]})

attempts = go.Figure()
for preset in presets:
    group = [r for r in runs if r['preset']==preset]
    attempts.add_trace(go.Scatter(x=[r['index'] for r in group], y=[labels[preset]]*len(group), mode='markers',
        marker={'size':20,'color':[colors[preset] if r['protocol_checks_passed'] else '#A33A21' for r in group],
                'symbol':['circle' if r['protocol_checks_passed'] else 'x' for r in group]},
        customdata=['Completed capture' if r['protocol_checks_passed'] else 'Client crash; no FPS score' for r in group],
        showlegend=False, hovertemplate='Attempt %{x}<br>%{y}<br>%{customdata}<extra></extra>'))
attempts.update_layout(template='plotly_white', height=220, margin={'l':180,'r':25,'t':20,'b':50},
    font={'family':'system-ui, sans-serif'},xaxis={'title':'Planned attempt order','tickmode':'linear','dtick':1,'range':[.5,8.5]},
    yaxis={'categoryorder':'array','categoryarray':[labels[p] for p in reversed(presets)]})

fragments = [summary.to_html(full_html=False, include_plotlyjs=True, config=config, div_id='fps-summary'),
             frames.to_html(full_html=False, include_plotlyjs=False, config=config, div_id='frame-spikes'),
             attempts.to_html(full_html=False, include_plotlyjs=False, config=config, div_id='attempts')]
rows = []
for r in runs:
    m = r['diagnostic_metrics'] if r['protocol_checks_passed'] else None
    vals = [f"{m['average_fps']:.1f}", f"{m['one_percent_low_fps']:.1f}"] if m else ['—','—']
    rel = Path(r['directory']).relative_to(root).as_posix()
    rows.append(f'<tr><td>{r["index"]}</td><td>{html.escape(labels[r["preset"]])}</td><td>{vals[0]}</td><td>{vals[1]}</td><td>{"Completed" if m else "Client crash"}</td><td><a href="{rel}/run-ledger.json">Ledger</a></td></tr>')
doc = '''<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Sqooky vs Dacooder · benchmark visualization</title><style>
*{box-sizing:border-box}body{margin:0;background:#f3f5f7;color:#1d2938;font:16px/1.55 system-ui,sans-serif}
main{max-width:1180px;margin:auto;padding:36px 20px}h1{font-size:clamp(27px,4vw,40px);line-height:1.15;margin:10px 0 14px}h2{font-size:22px;margin:0 0 6px}
.eyebrow{font-size:12px;font-weight:750;letter-spacing:.13em;color:#586579}.muted{color:#566374}.card{background:white;border:1px solid #dfe5eb;border-radius:14px;padding:24px;margin:22px 0}
.stats{display:grid;grid-template-columns:repeat(3,1fr);gap:14px}.stat{padding:18px 22px;background:white;border:1px solid #dfe5eb;border-radius:12px}.stat strong{display:block;font-size:29px}.stat span{font-size:13px;color:#566374}
.note{border-left:4px solid #ba7a21;padding:12px 16px;background:#fff9ee;font-size:14px}.plot{min-width:0}.scroll{overflow-x:auto}table{border-collapse:collapse;width:100%;font-size:14px}th,td{padding:10px;border-bottom:1px solid #e4e8ed;text-align:left}a{color:#245f9b}footer{font-size:13px;color:#566374;margin-top:25px}
@media(max-width:650px){main{padding:22px 10px}.card{padding:16px 8px}.stats{grid-template-columns:1fr}.stat strong{font-size:24px}}
</style></head><body><main>
<div class="eyebrow">LOCAL BENCHMARK · EXPLORATORY RESULTS</div>
<h1>Sqooky vs Dacooder: effectively tied in average FPS</h1>
<p class="muted">Ryzen 7 9800X3D · RTX 5070 Ti · Linux/Proton · 2560×1440 · 100% render scale · uncapped.<br>Estimated 26-second fight windows exclude loading, warm-up and resets.</p>
<div class="stats"><div class="stat"><strong>+0.25%</strong><span>OptiLock median average FPS difference</span></div><div class="stat"><strong>+1.21%</strong><span>OptiLock median 1% low difference</span></div><div class="stat"><strong>7 / 8</strong><span>Completed captures in the final series</span></div></div>
<section class="card"><h2>FPS and run-to-run variation</h2><p class="muted">Hover to inspect results. Bars show medians, dots show completed runs, and whiskers show observed min–max ranges, not confidence intervals. Higher is better.</p>SUMMARY</section>
<section class="card"><h2>Frame-time spikes</h2><p class="muted">Lower is better. Each line retains the worst frame in every 0.1-second bin. Click a legend item to hide it; double-click to isolate it. Drag the range slider to zoom.</p>FRAMES</section>
<section class="card"><h2>Every planned attempt</h2><p class="muted">Circles are completed captures. The cross is a client crash, with no FPS score. Sqooky completed 3/4; OptiLock completed 4/4 in this final series.</p>ATTEMPTS
<div class="scroll"><table><thead><tr><th>Attempt</th><th>Config</th><th>Average FPS</th><th>1% low FPS</th><th>Outcome</th><th>Evidence</th></tr></thead><tbody>ROWS</tbody></table></div></section>
<div class="note"><strong>Interpretation:</strong> The differences are small and run ranges overlap. Timing alignment is approximate, and the presets retain different visual-quality settings. VPK bundles were not installed. Both configs encountered the same client-module crash across the broader testing, including an earlier OptiLock attempt. These results do not establish a validated performance or stability ranking.</div>
<footer><a href="comparison-results.json">Complete results and settings hashes</a> · <a href="comparison-plan.json">Protocol and amendments</a> · <a href="crash-evidence/index.json">Crash evidence</a> · <a href="graphs/fps-comparison.svg">FPS chart SVG</a> · <a href="graphs/frame-time-spikes.svg">Frame-time chart SVG</a><p>Standalone, offline visualization. No data is uploaded. Original measurements are unchanged.</p></footer>
</main></body></html>'''
for marker in ['SUMMARY','FRAMES','ATTEMPTS','ROWS']:
    doc = doc.replace(marker, '<!-- VIEW_'+marker+' -->', 1)
for marker, value in zip(['SUMMARY','FRAMES','ATTEMPTS','ROWS'], fragments+[''.join(rows)]):
    doc = doc.replace('<!-- VIEW_'+marker+' -->',value,1)
output = root / 'comparison-visualization.html'
output.write_text(doc)
(root / 'graphs/interactive-manifest.json').write_text(json.dumps({
    'source_results_sha256':hashlib.sha256((root/'comparison-results.json').read_bytes()).hexdigest(),
    'html_sha256':hashlib.sha256(output.read_bytes()).hexdigest(), 'plotly_version':plotly.__version__,
    'output':str(output), 'figures':3, 'successful_traces':len(valid),
},indent=2)+'\n')
print(output)
