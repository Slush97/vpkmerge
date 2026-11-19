#!/usr/bin/env python3
"""Isolated config comparison with continuous capture and guarded extraction.

This is an experimental harness, not a validated score adapter. It reuses the
owned-process launcher and ledger; capture is a guarded interior fight window.
"""
import json
import csv
from datetime import datetime
from pathlib import Path
import re
import shutil
import socket
import subprocess
import sys
import threading
import time

RUNTIME = Path(__file__).resolve().parents[1] / 'runtime'
sys.path.insert(0, str(RUNTIME))
import play_lab
import benchmark_capture
from benchmark_recording import Recording
from benchmark_metrics import metrics


def main():
    root, preset, trial = Path(sys.argv[1]).resolve(), sys.argv[2], sys.argv[3]
    variant = root / 'variants' / preset
    if preset not in ('sqooky-default', 'optilock-fps'):
        raise ValueError('Unknown comparison preset')
    game = root / 'game'
    if (root / 'active.json').exists():
        raise RuntimeError('An owned session is already active')
    cfg = game / 'game/citadel/cfg'
    if cfg.is_symlink():
        raise RuntimeError('Isolated cfg directory must not be a symlink')
    if cfg.exists():
        shutil.rmtree(cfg)
    shutil.copytree(root / 'baseline-cfg', cfg)
    shutil.copyfile(variant / 'game/citadel/gameinfo.gi', game / 'game/citadel/gameinfo.gi')
    telemetry = {'preset': preset, 'trial': trial, 'trigger': 'continuous autostart; guarded offline extraction',
                 'scope': '26-second interior of measured fight, nominal +2s to +28s',
                 'alignment_verified': False, 'controls': [], 'focus_checks': [], 'errors': []}
    current = {}
    worker = None
    cancelled = threading.Event()
    original_video = play_lab.configure_video
    original_init = Recording.__init__
    original_marker = Recording.marker
    original_finish = benchmark_capture.Capture.finish
    original_popen = subprocess.Popen
    client_controls = ['-netconport', '21235', '+fps_max', '0', '+mat_vsync', '0',
                       '+mat_viewportscale', '1', '+r_citadel_upscaling', '0', '+r_aspectratio', '0']

    def configure(citadel, retail, run, resolution):
        flags = original_video(citadel, variant, run, 'game')
        return flags + ['-windowed', '-noborder', '-w', '2560', '-h', '1440']

    def popen(command, *args, **kwargs):
        role = None
        if isinstance(command, list) and 'run' in command and any('deadlock.exe' in str(x) for x in command):
            command = command + client_controls
            telemetry['client_command'] = command
            role = 'client'
        elif isinstance(command, list) and 'dev' in command and 'modlock' in str(command[0]):
            role = 'server'
        process = original_popen(command, *args, **kwargs)
        if role:
            poll = process.poll
            seen = False
            def observed_poll():
                nonlocal seen
                code = poll()
                if code is not None and not seen:
                    seen = True
                    telemetry.setdefault('process_exits', []).append({'role': role, 'code': code, 'wall': time.time()})
                return code
            process.poll = observed_poll
        return process

    def initialize(self, run, args, game, project):
        original_init(self, run, args, game, project)
        current.update(run=run, recording=self)
        console = game / 'game/citadel/console.log'
        if console.exists():
            console.rename(run / 'prior-client-console.log')
        shutil.copyfile(variant / 'preset.json', run / 'treatment.json')
        (run / 'comparison-protocol.json').write_text(json.dumps({
            'scope': telemetry['scope'], 'capture_start_guard_seconds': 2,
            'capture_stop_guard_seconds': 2, 'capture_duration_seconds': 26,
            'alignment_verified': False, 'preset': preset, 'trial': trial,
            'client_controls': client_controls, 'ui_port': 0, 'preview_port': 0,
            'cache_policy': 'fresh private server cache and Proton prefixes per launch; shared OS/driver caches',
            'harness_source': Path(__file__).read_text(),
        }, indent=2) + '\n')

    def focus_record():
        c = json.loads(subprocess.check_output(['hyprctl', 'activewindow', '-j'], timeout=3))
        return {'wall_time': time.time(), 'class': c.get('class'), 'title': c.get('title'),
                'size': c.get('size'), 'focused_game': c.get('title') == 'Deadlock'}

    def warmup_inspection():
        try:
            clients = json.loads(subprocess.check_output(['hyprctl', 'clients', '-j'], timeout=3))
            games = [c for c in clients if c.get('title') == 'Deadlock']
            if len(games) != 1:
                raise RuntimeError(f'Expected one game window, found {len(games)}')
            subprocess.run(['hyprctl', 'dispatch', 'focuswindow', 'address:' + games[0]['address']],
                           check=True, stdout=subprocess.DEVNULL, timeout=3)
            telemetry['warmup_window'] = {k: games[0].get(k) for k in ('size', 'pid', 'class', 'title')}
            with socket.create_connection(('127.0.0.1', 21235), timeout=3) as nc:
                nc.settimeout(.1)
                data = b''
                for command in ['echo BENCH_CONFIG_READBACK', 'fps_max']:
                    nc.sendall((command + '\n').encode())
                    until = time.monotonic() + .25
                    while time.monotonic() < until:
                        try:
                            part = nc.recv(65536)
                            if not part:
                                break
                            data += part
                        except socket.timeout:
                            pass
                (current['run'] / 'client-setting-readback.log').write_bytes(data)
            # Capture only the game window, outside the measured interval.
            if shutil.which('import'):
                # Reuse the inspected local helper that finds only the Deadlock X window.
                helper = Path('/home/esoc/.cache/deadlock-benchmark-replay/night-shift-20261106/monitor_runtime.py').read_text().split('state_path=')[0]
                ns = {}; exec(helper, ns)
                window = ns['find'](ns['x'].XDefaultRootWindow(ns['d']))
                if window:
                    subprocess.run(['import', '-window', str(window), str(current['run'] / 'warmup-camera.png')],
                                   check=True, timeout=10)
        except Exception as error:
            telemetry['errors'].append('warmup inspection: ' + str(error))

    def collect(marker_wall):
        try:
            start_wall = marker_wall + 2
            if time.time() > start_wall:
                raise RuntimeError('Start marker arrived too late for capture guard')
            if cancelled.wait(max(0, start_wall - time.time())):
                return
            telemetry['controls'].append({'event': 'analysis_window_start', 'wall': time.time(), 'monotonic': time.monotonic()})
            print('CONFIG_WINDOW_STARTED ' + json.dumps({'preset': preset, 'trial': trial}), flush=True)
            while True:
                telemetry['focus_checks'].append(focus_record())
                if cancelled.wait(min(1, max(0, marker_wall + 28 - time.time()))):
                    return
                if time.time() >= marker_wall + 28:
                    break
            telemetry['controls'].append({'event': 'analysis_window_end', 'wall': time.time(), 'monotonic': time.monotonic()})
            print('CONFIG_WINDOW_ENDED ' + json.dumps({'preset': preset, 'trial': trial}), flush=True)
        except Exception as error:
            telemetry['errors'].append('capture: ' + str(error))

    def marker(self, line):
        nonlocal worker
        original_marker(self, line)
        if 'REPLAY PROBE WARMUP:' in line:
            warmup_inspection()
        if 'REPLAY PROBE BENCH START' in line:
            wall = re.search(r'wall_time_ms=(\d+)', line)
            if not wall:
                telemetry['errors'].append('Start marker lacks wall time')
                return
            marker_wall = int(wall[1]) / 1000
            telemetry['start_marker'] = {'line': line, 'source_wall': marker_wall,
                                         'receipt_wall': time.time()}
            worker = threading.Thread(target=collect, args=(marker_wall,), daemon=True)
            worker.start()
        if 'REPLAY PROBE BENCH END' in line:
            telemetry['end_marker'] = {'line': line, 'receipt_wall': time.time()}

    def finish(self):
        cancelled.set()
        if worker:
            worker.join(timeout=3)
        result = original_finish(self)
        for source, name in [('console.log', 'client-console.log'),
                             ('gameinfo.gi', 'effective-gameinfo.gi'),
                             ('cfg/video.txt', 'video-after.txt')]:
            path = game / 'game/citadel' / source
            if path.is_file():
                shutil.copyfile(path, self.run / name)
        result['comparison_protocol'] = 'continuous capture; guarded estimated window; alignment unverified'
        if result.get('status') == 'captured' and 'start_marker' in telemetry:
            raw = [p for p in self.directory.glob('deadlock*.csv') if not p.name.endswith('_summary.csv')]
            stamp = re.search(r'(\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2})', raw[0].name)
            if not stamp:
                telemetry['errors'].append('Missing MangoHud start timestamp')
            else:
                # v0.8.4 Logger::start_logging resets elapsed time and names its
                # stream using localtime(time(NULL)). Use the middle of that
                # second and report +/-0.5s sensitivity. This is an estimate,
                # not a validated render-frame clock mapping or hard bound.
                epoch = datetime.strptime(stamp[1], '%Y-%m-%d_%H-%M-%S').timestamp() + .5
                start = telemetry['start_marker']['source_wall'] + 2 - epoch
                end = start + 26
                data = [(float(r['timestamp_seconds']), float(r['frame_time_ms']))
                        for r in csv.DictReader((self.directory / 'frames.csv').open())]
                def window(shift):
                    return [(t, ms) for t, ms in data if t - ms/1000 >= start + shift and t <= end + shift]
                windows = {str(shift): window(shift) for shift in [-.5, 0, .5]}
                if any(not v or v[-1][0] - v[0][0] < 25.5 for v in windows.values()):
                    telemetry['errors'].append('Insufficient frame coverage in a guarded window')
                else:
                    summaries = {k: metrics(v, sum(ms for _, ms in v)/1000) for k,v in windows.items()}
                    result['fight_metrics'] = summaries['0']
                    result['fight_window'] = {'scope': telemetry['scope'], 'capture_seconds': [start, end],
                        'filename_epoch_midpoint': epoch, 'alignment_verified': False,
                        'time_origin_method': 'MangoHud filename second midpoint; source wall marker; approximate',
                        'sensitivity_seconds': [-.5, 0, .5], 'sensitivity_metrics': summaries,
                        'hitch_counts': {str(ms): sum(ft > ms for _,ft in windows['0']) for ms in [16.666666667, 33.333333333, 50, 100]}}
                    with (self.directory / 'fight-window-frames.csv').open('w') as f:
                        writer = csv.writer(f); writer.writerow(['timestamp_seconds','frame_time_ms']); writer.writerows(windows['0'])
                    (self.directory / 'fight-window.json').write_text(json.dumps(result['fight_window'], indent=2)+'\n')
        else:
            telemetry['errors'].append('Missing captured frame stream or start marker')
        if any(not c['focused_game'] or c['size'] != [2560, 1440] for c in telemetry['focus_checks']):
            telemetry['errors'].append('Foreground focus or window size changed during sampled checks')
        if len(telemetry['controls']) == 2:
            a,b = telemetry['controls']
            if abs((b['wall']-a['wall']) - (b['monotonic']-a['monotonic'])) > .05:
                telemetry['errors'].append('Wall clock changed relative to monotonic clock')
        result['protocol_checks_passed'] = not telemetry['errors'] and len(telemetry['controls']) == 2 and 'fight_metrics' in result
        (self.run / 'trial-observation.json').write_text(json.dumps(telemetry, indent=2) + '\n')
        (self.directory / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        return result

    play_lab.configure_video = configure
    subprocess.Popen = popen
    Recording.__init__ = initialize
    Recording.marker = marker
    benchmark_capture.Capture.finish = finish
    sys.argv = [str(RUNTIME / 'play_lab.py'), 'start', '--game', str(game),
                '--scenario', str(root / 'scenario'), '--state-dir', str(root),
                '--modlock', '/home/esoc/.cache/deadlock-benchmark-runtimes/v0.1.74/modlock-linux-amd64',
                '--host', '/home/esoc/.cache/deadlock-benchmark-runtimes/v0.1.74/modlock-host.exe',
                '--demo', '--supervise', '--ui-port', '0', '--capture', 'mangohud',
                '--resolution', 'game', '--suite', 'sqooky-vs-optilock-main', '--label', f'{preset}-{trial}']
    return play_lab.main()


if __name__ == '__main__':
    raise SystemExit(main())
