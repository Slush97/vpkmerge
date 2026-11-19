import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location('replay_prepare', Path(__file__).resolve().parents[1] / 'replay/prepare.py')
replay = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(replay)


def sample(x=0, yaw=0, health=100):
    return {'position_x': x, 'position_y': 0, 'position_z': 0,
            'facing_y': yaw, 'health': health,
            'has_position': True, 'has_facing': True, 'has_health': True}


class ReplayTests(unittest.TestCase):
    def test_recast_restores_recorded_base_tiers_in_action_slot(self):
        def field(v): return {'value': v, 'present': True}
        base = {'class_name': 'CCitadel_Ability_TangoTether', 'slot': field(23),
                'subclass_id': field(42), 'upgrade_info': field(983040)}
        recast = {'class_name': 'CCitadel_Ability_TangoTether_Trigger', 'slot': field(1),
                  'subclass_id': field(43), 'upgrade_info': field(65536)}
        targets, resolved = replay.equipment_targets([base, recast], True)
        self.assertEqual(targets, [{'id': 42, 'slot': 1, 'state': 983040}])
        self.assertEqual(resolved[0]['recast_id'], 43)
        for items in ([recast], [base, base, recast]):
            with self.assertRaisesRegex(ValueError, 'recorded base'):
                replay.equipment_targets(items, True)

    def test_sparse_dead_pawn_requires_dead_end_state_and_same_handle(self):
        dead = dict(sample(health=0), entity=42, entity_serial=3, has_life_state=True, life_state=2)
        rows, count = replay.hold_dead_samples({10: dead, 18: dead}, 10, 20, dead)
        self.assertEqual(count, 9)
        track, filled = replay.make_track(rows, 10, 20)
        self.assertEqual(track['health'], [0] * 11)
        self.assertEqual(filled, 0)
        for final in (None, dict(dead, health=100), dict(dead, entity_serial=4), dict(dead, has_life_state=False)):
            rows, _ = replay.hold_dead_samples({10: dead}, 10, 20, final)
            self.assertNotIn(20, rows)
        rows, count = replay.hold_dead_samples({10: sample(), 20: sample()}, 10, 20)
        self.assertEqual(count, 0)
        with self.assertRaisesRegex(ValueError, 'gap'):
            replay.make_track(rows, 10, 20)
        rows, count = replay.hold_dead_samples({10: dead, 200: dead}, 10, 200)
        self.assertEqual(count, 0)

    def test_full_packet_refresh_preserves_sample_state(self):
        a = dict(sample(), tick=100, entity=42, entity_serial=3, position_x_tick=90)
        b = dict(a, position_x_tick=100, camera_angles_ticks=[100, 100, 100])
        self.assertTrue(replay.same_sample_state(a, b))
        for field, value in [('position_x', 1), ('health', 0), ('entity_serial', 4),
                             ('tick', 101), ('has_position', False)]:
            with self.subTest(field=field):
                self.assertFalse(replay.same_sample_state(a, dict(b, **{field: value})))

    def test_invalid_repetition_count_fails_before_reading_or_writing(self):
        for count in (0, -1, 11, 1.5, True):
            with self.subTest(count=count), self.assertRaisesRegex(ValueError, 'repetitions'):
                replay.package(None, None, None, repetitions=count)

    def test_cast_slot_zero_requires_presence_flag(self):
        event = {'tick': 120, 'player_slot': 1, 'ability_cast': {
            'has_ability_slot': True, 'has_subclass_id': True, 'subclass_id': 42, 'class_name': 'TestAbility'}}
        self.assertEqual(replay.hero_cast(event, 100, {1})['abilitySlot'], 0)
        event['ability_cast']['has_ability_slot'] = False
        self.assertIsNone(replay.hero_cast(event, 100, {1}))

    def test_movement_and_unattributed_casts_are_not_hero_inputs(self):
        event = {'tick': 120, 'player_slot': 1, 'ability_cast': {
            'has_ability_slot': True, 'ability_slot': 17, 'has_subclass_id': True,
            'subclass_id': 42, 'class_name': 'Dash'}}
        self.assertIsNone(replay.hero_cast(event, 100, {1}))
        event['ability_cast']['ability_slot'] = 0
        event['player_slot'] = -1
        self.assertIsNone(replay.hero_cast(event, 100, {1}))

    def test_interpolation_wraps_angles_and_keeps_health_steps(self):
        track, filled = replay.make_track({10: sample(0, 179), 12: sample(20, -179, 0)}, 10, 12)
        self.assertEqual(track['x'], [0, 10, 20])
        self.assertEqual(track['yaw'], [179, 180, -179])
        self.assertEqual(track['health'], [100, 100, 0])
        self.assertEqual(filled, 1)

    def test_omitted_observed_zero_angle_is_valid(self):
        row = sample()
        del row['facing_y']
        track, _ = replay.make_track({10: row, 11: row}, 10, 11)
        self.assertEqual(track['yaw'], [0, 0])

    def test_missing_facing_is_not_zero(self):
        row = sample()
        row['has_facing'] = False
        with self.assertRaisesRegex(ValueError, 'missing pose'):
            replay.make_track({10: row, 11: row}, 10, 11)

    def test_incomplete_boundaries_and_large_gaps_are_rejected(self):
        for rows, start, end in [({11: sample(), 12: sample()}, 10, 12),
                                 ({10: sample(), 16: sample()}, 10, 16)]:
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                replay.make_track(rows, start, end)

    def test_nonfinite_track_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'non-finite'):
            replay.make_track({10: sample(float('nan')), 11: sample()}, 10, 11)

    def test_missing_snapshot_field_is_not_zero(self):
        with self.assertRaisesRegex(ValueError, 'missing initial'):
            replay.value({'present': False, 'value': 0}, 'health')
        self.assertEqual(replay.value({'present': True, 'value': 0}, 'health'), 0)

    def test_window_separates_spatially_distinct_fights(self):
        hits = []
        for tick in range(5, 51):
            for offset, slots in [(0, (1, 2)), (10000, (3, 4))]:
                hits.append({'tick': tick, 'attacker_slot': slots[0], 'victim_slot': slots[1],
                             'attacker_position': [offset, 0, 0], 'victim_position': [offset, 20, 0], 'damage': 10})
        chosen = replay.choose_window(hits, 1, seconds=30, lead=5, radius=2500)
        self.assertEqual(len(chosen['combat_slots']), 2)
        self.assertEqual(chosen['end_tick'] - chosen['measurement_start_tick'], 30)
        self.assertEqual(chosen['measurement_start_tick'] - chosen['start_tick'], 5)

    def test_empty_and_out_of_order_damage_are_rejected(self):
        with self.assertRaises(ValueError):
            replay.choose_window([], 1 / 64)
        with self.assertRaisesRegex(ValueError, 'monotonic'):
            replay.choose_window([{'tick': 12}, {'tick': 11}], 1 / 64)


if __name__ == '__main__':
    unittest.main()
