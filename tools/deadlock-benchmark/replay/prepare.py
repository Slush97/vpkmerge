#!/usr/bin/env python3
"""Select a spatial combat window and package observed replay tracks for Modlock."""
import argparse
from bisect import bisect_right
from collections import Counter, defaultdict
import hashlib
import json
import math
from pathlib import Path
import shutil

HERE = Path(__file__).resolve().parent


def read_json(path):
    return json.loads(path.read_text())


def write_json(path, value):
    path.write_text(json.dumps(value, separators=(',', ':'), allow_nan=False) + '\n')


def choose_window(hits, interval, seconds=30, lead=5, radius=2500):
    """Rank local damage clusters, not simultaneous fights across the map."""
    if not hits or not math.isfinite(interval) or interval <= 0:
        raise ValueError('need observed damage and a positive recorded tick interval')
    if seconds <= 0 or lead < 0 or radius <= 0:
        raise ValueError('invalid window, lead-in or radius')
    times = [h['tick'] * interval for h in hits]
    if times != sorted(times):
        raise ValueError('damage ticks are not monotonic')
    best = None
    for start in range(math.ceil(max(lead, times[0]) / 5) * 5, math.floor(times[-1] - seconds) + 1, 5):
        window = hits[bisect_right(times, start - 1e-9):bisect_right(times, start + seconds - 1e-9)]
        for anchor in window[::max(1, len(window) // 10)]:
            pos = anchor['victim_position']
            near = [h for h in window if math.dist(h['victim_position'], pos) < radius
                    and math.dist(h['attacker_position'], pos) < radius]
            slots = sorted({s for h in near for s in (h['attacker_slot'], h['victim_slot'])})
            score = (len(slots), len(near), sum(h['damage'] for h in near))
            if best is None or score > best[0]:
                best = (score, {'start_tick': round((start - lead) / interval),
                               'measurement_start_tick': round(start / interval),
                               'end_tick': round((start + seconds) / interval),
                               'lead_in_seconds': lead, 'duration_seconds': seconds,
                               'anchor': pos, 'radius_units': radius, 'combat_slots': slots,
                               'local_damage_events': len(near), 'local_damage': score[2],
                               'time_basis': 'demo ticks; not the displayed match clock'})
    if best is None:
        raise ValueError('no complete combat window')
    return best[1]


def value(field, label):
    if not field.get('present'):
        raise ValueError(f'missing initial {label}')
    n = field['value']
    if not math.isfinite(n):
        raise ValueError(f'non-finite {label}')
    return n


def make_track(rows, start, end):
    """Keep every source tick; interpolate only bounded gaps of at most 4 ticks."""
    ticks = sorted(rows)
    if not ticks or ticks[0] != start or ticks[-1] != end:
        raise ValueError('actor lacks an exact start or end sample')
    if any(b - a > 4 for a, b in zip(ticks, ticks[1:])):
        raise ValueError('actor sample gap exceeds four ticks')
    keys = {'x': 'position_x', 'y': 'position_y', 'z': 'position_z',
            'yaw': 'facing_y', 'pitch': 'facing_x', 'health': 'health'}
    track = {'stepTicks': 1, **{k: [] for k in keys}}
    interpolated = 0
    for tick in range(start, end + 1):
        left = ticks[bisect_right(ticks, tick) - 1]
        right = ticks[min(bisect_right(ticks, tick), len(ticks) - 1)]
        a, b = rows[left], rows[right]
        if not a.get('has_position') or not a.get('has_facing') or not a.get('has_health'):
            raise ValueError(f'missing pose or health at tick {left}')
        weight = (tick - left) / (right - left) if right != left else 0
        if tick not in rows:
            interpolated += 1
        for key, source in keys.items():
            # s2replay omits observed zero-valued angles, distinguished by has_facing.
            av, bv = a.get(source, 0), b.get(source, 0)
            if not math.isfinite(av) or not math.isfinite(bv):
                raise ValueError('non-finite track')
            delta = (bv - av + 180) % 360 - 180 if key in ('yaw', 'pitch') else bv - av
            # Death and health changes are steps, not interpolated source evidence.
            n = av if key == 'health' else av + delta * weight
            track[key].append(round(n, 5))
    return track, interpolated


def hero_cast(event, start, slots):
    a = event['ability_cast']
    slot = event['player_slot']
    # An omitted zero is valid only when the accompanying presence bit is true.
    ability_slot = a.get('ability_slot', 0)
    if slot not in slots or not a.get('has_ability_slot') or not 0 <= ability_slot < 4 or not a.get('has_subclass_id'):
        return None
    return {'tick': event['tick'] - start, 'slot': slot, 'abilitySlot': ability_slot,
            'id': a['subclass_id'], 'className': a['class_name']}


def same_sample_state(a, b):
    # Full packets refresh per-field observation timestamps without changing
    # the actor state. Keep tick, handles, presence flags and all values strict.
    def state(sample):
        return {k: v for k, v in sample.items() if not k.endswith(('_tick', '_ticks'))}
    return state(a) == state(b)


def hold_dead_samples(rows, start, end, final=None):
    """Hold dead poses only between confirmed zero-health states of one pawn."""
    result = dict(rows)
    def dead(s):
        return (s.get('has_health') and s.get('health', 0) == 0
                and s.get('has_life_state') and s.get('life_state') == 2)
    def same_pawn(a, b):
        return (a.get('entity'), a.get('entity_serial', 0)) == (b.get('entity'), b.get('entity_serial', 0))
    if end not in result and result and final is not None:
        last = result[max(result)]
        if end - max(result) <= 128 and dead(last) and dead(final) and same_pawn(last, final):
            result[end] = final
    ticks = sorted(result)
    for left, right in zip(ticks, ticks[1:]):
        a, b = result[left], result[right]
        if right - left <= 128 and dead(a) and dead(b) and same_pawn(a, b):
            for tick in range(left + 1, right):
                result[tick] = a
    return result, len(result) - len(rows)


def equipment_targets(items, abilities=False):
    # These recast buttons temporarily occupy the base ability's action slot.
    # The same snapshot retains the base definition and its purchased tiers.
    recasts = {'CCitadel_Ability_TangoTether_Trigger': 'CCitadel_Ability_TangoTether',
               'CAbility_Synth_PlasmaFlux_Trigger': 'CAbility_Synth_PlasmaFlux'}
    targets, resolutions = [], []
    for item in items:
        slot = value(item['slot'], 'equipment slot')
        if abilities and slot >= 4:
            continue
        definition = item
        base = recasts.get(item.get('class_name')) if abilities else None
        if base:
            candidates = [i for i in items if i.get('class_name') == base]
            if len(candidates) != 1:
                raise ValueError('recast ability requires one recorded base definition')
            definition = candidates[0]
            resolutions.append({'slot': slot, 'recast_id': value(item['subclass_id'], 'recast ID'),
                                'base_id': value(definition['subclass_id'], 'base ID'),
                                'base_class': base})
        targets.append({'id': value(definition['subclass_id'], 'equipment ID'), 'slot': slot,
                        'state': value(definition['upgrade_info'], 'upgrade state')})
    return targets, resolutions


def package(clip, selection, output, adapt_current_build=False, motion_only=False, repetitions=1):
    if type(repetitions) is not int or not 1 <= repetitions <= 10:
        raise ValueError('repetitions must be an integer from 1 to 10')
    meta = read_json(clip / 'manifest.json')
    state = read_json(clip / 'initial-state.json')
    start, end = meta['start_tick'], meta['end_tick']
    if (selection['start_tick'], selection['end_tick']) != (start, end):
        raise ValueError('selection and clip bounds differ')
    if selection.get('source_sha256', meta['source_sha256']) != meta['source_sha256']:
        raise ValueError('selection and clip source hashes differ')
    if state['source']['sha256'] != meta['source_sha256']:
        raise ValueError('initial state and clip source hashes differ')
    if not meta['tick_interval_known'] or meta['tick_interval_seconds'] != 1 / 64:
        raise ValueError('this Modlock Puppet adapter requires an observed 64 Hz source')
    initial = {h['player_slot']: h for h in state['heroes']}
    if len(initial) != 12:
        raise ValueError('expected all 12 recorded participants')
    handles = {(h['entity_id'], h['entity_serial']): slot for slot, h in initial.items()}
    rows = defaultdict(dict)
    casts, raw_counts = [], Counter()
    attributed = 0
    repeated_samples = 0
    previous = start
    for line in (clip / 'events.jsonl').open():
        event = json.loads(line)
        tick = event['tick']
        if tick < previous or not start <= tick <= end:
            raise ValueError('nonmonotonic or out-of-window event')
        previous = tick
        raw_counts[event['type']] += 1
        if event['type'] == 'entity_sample':
            s = event['entity_sample']
            handle = (s['entity'], s.get('entity_serial', 0))
            slot = event['player_slot']
            if slot < 0:
                slot = handles.get(handle, -1)
                attributed += slot >= 0
            if slot not in initial:
                continue
            if handle in handles and handles[handle] != slot:
                raise ValueError('conflicting pawn identity')
            handles[handle] = slot
            if s.get('hero_id') != value(initial[slot]['hero_id'], 'hero ID'):
                raise ValueError('hero identity changes within clip')
            if tick in rows[slot]:
                if not same_sample_state(rows[slot][tick], s):
                    raise ValueError('conflicting duplicate sample')
                repeated_samples += 1
            rows[slot][tick] = s
        elif event['type'] == 'ability_cast':
            cast = hero_cast(event, start, initial)
            if cast is not None:
                casts.append(cast)
    actors = []
    interpolation = {}
    dead_holds = {}
    recast_resolutions = {}
    final = read_json(clip / 'final-state.json') if (clip / 'final-state.json').exists() else None
    if final is not None and final['tick'] != end:
        raise ValueError('final state and clip bounds differ')
    for slot, h in sorted(initial.items()):
        samples, held = hold_dead_samples(rows[slot], start, end,
                                         final['heroes'].get(str(slot)) if final else None)
        track, filled = make_track(samples, start, end)
        dead_holds[str(slot)] = held
        interpolation[str(slot)] = filled
        abilities, resolved = equipment_targets(h['abilities'], True)
        items, _ = equipment_targets(h['items'])
        recast_resolutions[str(slot)] = resolved
        actors.append({'slot': slot, 'hero': value(h['hero_id'], 'hero ID'),
                       'team': value(h['team'], 'team'), 'level': value(h['level'], 'level'),
                       'maxHealth': value(h['max_health'], 'max health'),
                       'abilities': abilities, 'items': items,
                       'recastBases': [r['base_id'] for r in resolved],
                       'track': track})
    survivors = [a for a in actors if min(a['track']['health']) > 0]
    if not survivors:
        raise ValueError('no continuously alive actor for the chase-camera reference')
    def camera_distance(actor):
        t = actor['track']
        return sum(math.dist((x, y, z), selection['anchor']) for x, y, z in zip(t['x'], t['y'], t['z'])) / len(t['x'])
    camera_actor = min(survivors, key=camera_distance)
    manifest = {'schema_version': 1, 'source': state['source'], 'selection': selection,
                'cameraSlot': camera_actor['slot'],
                'adaptCurrentBuild': adapt_current_build,
                'motionOnly': motion_only,
                'tick_interval_seconds': 1 / 64, 'ticks': end - start,
                'leadInTicks': selection['measurement_start_tick'] - start,
                'repetitions': repetitions,
                'actors': [{k: v for k, v in a.items() if k != 'track'} for a in actors],
                'casts': casts, 'counts': dict(raw_counts),
                'interpolated_ticks_per_actor': interpolation,
                'samples_attributed_by_initial_handle': attributed,
                'equivalent_duplicate_samples': repeated_samples,
                'dead_pose_held_ticks_per_actor': dead_holds,
                'recast_base_ability_resolutions': recast_resolutions,
                'match_clock_at_clip_start': state['tick_provenance']['match_clock_seconds'],
                'valid_benchmark': False,
                'limitations': [
                    'Source replay/runtime compatibility must be checked separately.',
                    'Adapted mode uses current hero abilities by slot and no items, not the recorded builds.',
                    'Motion is forced to recorded paths; velocities across samples are derived.',
                    'Ability timing is inferred from cooldown transitions; casts must be observed at runtime.',
                    'Weapon fire is synthetic continuous fire with periodic reload; no recorded input stream exists.',
                    'Recorded health is imposed; live damage is suppressed to prevent extra deaths.',
                    'Sparse dead-pawn samples hold the last pose between confirmed dead states; corpse motion is not reconstructed.',
                    'Shields, active modifiers, exact cooldowns and world/NPC state are not restored.',
                    'Known recast buttons restore their recorded base abilities and tiers; their already-active effects are not recreated.',
                    'Current map geometry and current hero/item definitions are used.',
                    'The observer follows a recorded survivor with the native chase camera.',
                    'Motion-only mode omits combat and recorded health/deaths.',
                    'Camera, visible effects, capture alignment and repeatability require validation.']}
    output.mkdir(parents=True, exist_ok=False)
    shutil.copytree(HERE / 'scenario', output, dirs_exist_ok=True,
                    ignore=shutil.ignore_patterns('build', 'node_modules'))
    (output / 'data').mkdir()
    for actor in actors:
        write_json(output / 'data' / f"actor-{actor['slot']}.json", actor['track'])
    write_json(output / 'data/clip.json', manifest)
    manifest['source_files'] = {name: hashlib.sha256((clip / name).read_bytes()).hexdigest()
                                for name in ('manifest.json', 'initial-state.json', 'events.jsonl')}
    if final is not None:
        manifest['source_files']['final-state.json'] = hashlib.sha256((clip / 'final-state.json').read_bytes()).hexdigest()
    write_json(output / 'provenance.json', manifest)
    return manifest


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest='action', required=True)
    select = sub.add_parser('select')
    select.add_argument('scan', type=Path)
    select.add_argument('--output', type=Path, required=True)
    bundle = sub.add_parser('package')
    bundle.add_argument('clip', type=Path)
    bundle.add_argument('--selection', type=Path, required=True)
    bundle.add_argument('--output', type=Path, required=True)
    bundle.add_argument('--adapt-current-build', action='store_true',
                        help='exploratory fallback: current abilities by slot, no recorded items')
    bundle.add_argument('--motion-only', action='store_true', help='diagnostic: paths only, no combat or recorded health/deaths')
    bundle.add_argument('--repetitions', type=int, default=1,
                        help='measured passes after one warm-up, with actor reset between passes (1–10)')
    args = p.parse_args()
    try:
        if args.output.exists():
            raise ValueError(f'output already exists: {args.output}')
        if args.action == 'select':
            meta = read_json(args.scan / 'manifest.json')
            if not meta['tick_interval_known']:
                raise ValueError('source tick interval is unknown')
            hits = [json.loads(line) for line in (args.scan / 'events.jsonl').open()]
            result = choose_window(hits, meta['tick_interval_seconds'])
            result['source_sha256'] = meta['source_sha256']
            write_json(args.output, result)
        else:
            result = package(args.clip, read_json(args.selection), args.output, args.adapt_current_build, args.motion_only, args.repetitions)
            result = {'actors': len(result['actors']), 'casts': len(result['casts']),
                      'interpolated_ticks_per_actor': result['interpolated_ticks_per_actor'],
                      'valid_benchmark': False}
        print(json.dumps(result, indent=2))
    except (OSError, ValueError, KeyError) as e:
        p.exit(1, f'{e}\n')


if __name__ == '__main__':
    main()
