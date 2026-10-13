"""Equalizer APO import/export, the profile library, and the Audeze guard. No HID devices are opened."""
import json
from pathlib import Path
import tempfile
import unittest

import apo
import server

SAMPLE = """# AutoEQ style preset
Preamp: -6.2 dB
Filter 1: ON LSC Fc 105 Hz Gain 6.0 dB Q 0.70
Filter 2: ON PK Fc 180 Hz Gain -2.4 dB Q 1.10
Filter 3: OFF PK Fc 6000 Hz Gain 1 dB Q 4
Filter 4: ON BP Fc 500 Hz Q 2
Filter 5: ON HPQ Fc 30 Hz Q 0.7
Device: Speakers
"""


class ApoImportTests(unittest.TestCase):
    def test_parses_supported_filters_and_reports_the_rest(self):
        result = apo.parse(SAMPLE)
        self.assertEqual([f['type'] for f in result['filters']], ['low_shelf', 'peak', 'peak', 'high_pass'])
        self.assertEqual(result['filters'][0]['q'], apo.SHELF_Q)
        self.assertFalse(result['filters'][2]['enabled'])
        self.assertEqual(result['filters'][3]['gain'], 0)
        self.assertEqual(result['preamp'], 'auto')
        self.assertEqual(result['source_preamp'], -6.2)
        warnings = ' '.join(result['warnings'])
        self.assertIn('band-pass', warnings)
        self.assertIn('Device', warnings)
        self.assertIn('Preamp -6.2', warnings)

    def test_clamps_out_of_range_values(self):
        result = apo.parse('Filter: ON PK Fc 25000 Hz Gain 20 dB Q 50')
        f = result['filters'][0]
        self.assertEqual((f['frequency'], f['gain'], f['q']), (20000, 12, 20))
        self.assertEqual(len(result['warnings']), 3)

    def test_bandwidth_converts_to_q(self):
        q = apo.parse('Filter 1: ON PK Fc 1000 Hz Gain -3 dB BW Oct 1')['filters'][0]['q']
        self.assertAlmostEqual(q, 1.414, places=3)

    def test_keeps_first_ten_filters(self):
        text = '\n'.join(f'Filter {i}: ON PK Fc {100 + i * 50} Hz Gain 1 dB Q 1' for i in range(1, 13))
        result = apo.parse(text)
        self.assertEqual(len(result['filters']), 10)
        self.assertIn('first 10 of 12', ' '.join(result['warnings']))

    def test_rejects_unusable_text(self):
        for text in ('', 'Preamp: -3 dB', 'GraphicEQ: 20 0; 40 1'):
            with self.subTest(text=text), self.assertRaises(ValueError):
                apo.parse(text)

    def test_export_round_trips(self):
        preset = dict(preamp='auto', filters=[
            dict(type='low_shelf', frequency=105, gain=5.5, q=apo.SHELF_Q, enabled=True),
            dict(type='peak', frequency=1450, gain=2.1, q=2, enabled=False),
            dict(type='low_pass', frequency=16000, gain=0, q=0.7071, enabled=True)])
        text = apo.export(preset, preamp_db=-7.6)
        self.assertTrue(text.startswith('Preamp: -7.6 dB\n'))
        self.assertIn('Filter 2: OFF PK Fc 1450 Hz Gain 2.1 dB Q 2', text)
        again = apo.parse(text)
        self.assertEqual([(f['type'], f['frequency'], f['gain'], f['enabled']) for f in again['filters']],
                         [(f['type'], f['frequency'], f['gain'], f['enabled']) for f in preset['filters']])


class ProfileLibraryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.path = Path(self.temporary.name) / 'profiles.json'
        self.preset = dict(filters=[dict(type='peak', frequency=1000, gain=-3, q=1, enabled=True)], preamp='auto')

    def test_starts_with_flat_and_persists_changes(self):
        library = server.Profiles(self.path)
        self.assertEqual(library.listing()['profiles'][0]['name'], 'Flat')
        made = library.perform('/api/profiles/save', dict(name='  Bass  ', preset=self.preset, activate=True))
        self.assertEqual(made['active'], made['saved'])
        reloaded = server.Profiles(self.path).listing()
        self.assertEqual([p['name'] for p in reloaded['profiles']], ['Flat', 'Bass'])
        self.assertEqual(reloaded['active'], made['saved'])

    def test_rename_keeps_preset_and_delete_moves_active(self):
        library = server.Profiles(self.path)
        saved = library.perform('/api/profiles/save', dict(name='Bass', preset=self.preset, activate=True))['saved']
        library.perform('/api/profiles/save', dict(id=saved, name='Warm'))
        profile = library.find(saved)
        self.assertEqual((profile['name'], profile['preset']['filters'][0]['gain']), ('Warm', -3))
        result = library.perform('/api/profiles/delete', dict(id=saved))
        self.assertEqual(result['active'], result['profiles'][0]['id'])
        with self.assertRaises(ValueError):
            library.perform('/api/profiles/delete', dict(id=result['active']))

    def test_rejects_bad_input(self):
        library = server.Profiles(self.path)
        for request in (dict(name='', preset=self.preset), dict(name='x' * 61, preset=self.preset),
                        dict(name='Bad', preset=dict(filters=[])),
                        dict(name='Bad', preset=dict(filters=[dict(type='notch', frequency=1, gain=0, q=1)])),
                        dict(name='Bad', preset=dict(self.preset, preamp='loud'))):
            with self.subTest(request=request), self.assertRaises(ValueError):
                library.perform('/api/profiles/save', request)

    def test_order_must_match_library(self):
        library = server.Profiles(self.path)
        second = library.perform('/api/profiles/save', dict(name='B', preset=self.preset))['saved']
        first = library.listing()['profiles'][0]['id']
        ordered = library.perform('/api/profiles/order', dict(ids=[second, first]))['profiles']
        self.assertEqual([p['id'] for p in ordered], [second, first])
        with self.assertRaises(ValueError):
            library.perform('/api/profiles/order', dict(ids=[second]))

    def test_drops_damaged_entries_on_load(self):
        self.path.write_text(json.dumps(dict(active='gone', profiles=[
            dict(id='a', name='Good', preset=self.preset), dict(id='b', name='Broken', preset=dict(filters='x'))])))
        listing = server.Profiles(self.path).listing()
        self.assertEqual([p['name'] for p in listing['profiles']], ['Good'])
        self.assertEqual(listing['active'], 'a')


class AudezeGuardTests(unittest.TestCase):
    def test_device_operations_refuse_while_audeze_runs(self):
        app = server.App(True, audeze=server.AudezeWatch(check=lambda: ['Audeze HQ.exe']))
        with self.assertRaisesRegex(ValueError, 'Audeze App/HQ is open'):
            app.perform('/api/apply', dict(filters=[], preamp='auto'))
        self.assertIs(app.status()['audeze_running'], True)
        self.assertEqual(app.status()['audeze_apps'], ['Audeze HQ.exe'])

    def test_status_reports_false_when_no_audeze_app_runs(self):
        # An empty list is truthy in JavaScript, so the page needs a real boolean.
        app = server.App(True, audeze=server.AudezeWatch(check=lambda: []))
        self.assertIs(app.status()['audeze_running'], False)

    def test_watch_caches_between_polls(self):
        calls = []
        watch = server.AudezeWatch(check=lambda: calls.append(1) or [], ttl=60)
        watch.running()
        watch.running()
        self.assertEqual(len(calls), 1)
        watch.running(fresh=True)
        self.assertEqual(len(calls), 2)


if __name__ == '__main__':
    unittest.main()
