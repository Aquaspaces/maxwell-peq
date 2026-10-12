"""Local profile persistence and read-only connection handoffs."""
import copy
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from server import App


class UsabilityTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        root = Path(self.directory.name)
        self.session = root / 'session.json'
        self.library = root / 'profiles.json'
        self.app = App(True, self.session, self.library)
        self.preset = dict(filters=[dict(type='peak', frequency=1370, gain=-12, q=0.35)], preamp='auto')

    def test_local_library_roundtrip_and_operations_never_touch_hardware(self):
        with patch('server.worker') as worker:
            self.app.enable_live = False
            saved = self.app.perform('/api/profiles/save', dict(name='Music', preset=self.preset))['profile']
            restored = App(False, self.session, self.library)
            self.assertEqual(restored.profiles.list(), [saved])
            renamed = restored.perform('/api/profiles/rename', dict(id=saved['id'], revision=1, name='Evening'))['profile']
            self.assertEqual(renamed['preset'], saved['preset'])
            self.assertEqual(renamed['revision'], 2)
            updated = restored.perform('/api/profiles/save', dict(id=saved['id'], revision=2, name='Evening',
                preset=dict(filters=[dict(type='high_pass', frequency=80)], preamp='auto')))['profile']
            self.assertEqual(updated['preset']['filters'][0]['type'], 'high_pass')
            restored.perform('/api/profiles/delete', dict(id=saved['id'], revision=3))
            self.assertEqual(App(False, self.session, self.library).profiles.list(), [])
            worker.assert_not_called()

    def test_stale_revision_and_duplicate_names_preserve_library(self):
        saved = self.app.perform('/api/profiles/save', dict(name='Music', preset=self.preset))['profile']
        for request in (dict(name=' music ', preset=self.preset),
                        dict(id=saved['id'], revision=0, name='Music', preset=self.preset)):
            with self.assertRaises(ValueError):
                self.app.perform('/api/profiles/save', request)
        self.assertEqual(self.app.profiles.list(), [saved])

    def test_invalid_profile_and_disk_failure_preserve_saved_data(self):
        saved = self.app.perform('/api/profiles/save', dict(name='../Profile', preset=self.preset))['profile']
        # Names are display data, never filesystem paths.
        self.assertTrue(self.library.is_file())
        invalid = dict(filters=[dict(frequency=1000, gain=float('nan'), q=1)])
        with self.assertRaises(ValueError):
            self.app.perform('/api/profiles/save', dict(name='Invalid', preset=invalid))
        with patch.object(self.app.profiles, 'write_json', side_effect=OSError('Disk full')):
            with self.assertRaises(OSError):
                self.app.perform('/api/profiles/delete', dict(id=saved['id'], revision=1))
        self.assertEqual(self.app.profiles.list(), [saved])

    def test_corrupt_library_does_not_block_device_recovery(self):
        self.setup_saved_session()
        self.app._persist()
        self.library.write_text('{bad json', encoding='utf-8')
        recovered = App(True, self.session, self.library)
        self.assertIsNone(recovered.profiles)
        self.assertIsNotNone(recovered.backup)
        with self.assertRaisesRegex(ValueError, 'profile library'):
            recovered.perform('/api/profiles/save', dict(name='Music', preset=self.preset))
        with patch('server.worker', return_value={}) as worker:
            recovered.perform('/api/restore', {})
            self.assertEqual(worker.call_args.args[1]['blob_hex'], '11' * 844)

    def setup_saved_session(self):
        original = dict(device=dict(path='usb-old', transport='usb'), index=10, slot=3,
                        raw_ui_hex='aa', saved_coefficients_hex='11' * 844)
        self.app.backup = dict(state=copy.deepcopy(original))
        self.app.backup_path = 'original-backup.json'
        self.app.expected = dict(original, saved_coefficients_hex='22' * 844)
        self.app.live_state = 'applied'
        self.app.storage_state = 'saved'
        target = copy.deepcopy(self.app.expected)
        target['device'] = dict(path='dongle-new', transport='dongle')
        return target

    def test_switch_preserves_original_and_restores_via_new_interface(self):
        target = self.setup_saved_session()
        original = copy.deepcopy(self.app.backup)
        with patch('server.worker', return_value=target) as worker:
            self.app.perform('/api/connect', dict(transport='dongle'))
            worker.assert_called_once_with('device_worker.py', dict(operation='read', transport='dongle'))
        restored = App(True, self.session, self.library)
        self.assertEqual(restored.backup, original)
        self.assertEqual(restored.expected, target)
        self.assertEqual(restored.transport, 'dongle')
        self.assertEqual(restored.storage_state, 'saved')
        self.assertEqual(restored.live_state, 'applied')
        with patch('server.worker', return_value={}) as worker:
            restored.perform('/api/restore', {})
            request = worker.call_args.args[1]
            self.assertEqual(request['transport'], 'dongle')
            self.assertEqual(request['expected']['device']['path'], 'dongle-new')
            self.assertEqual(request['blob_hex'], original['state']['saved_coefficients_hex'])

    def test_mismatch_or_disconnect_does_not_rebind_or_discard_backup(self):
        target = self.setup_saved_session()
        before = copy.deepcopy(self.app._session())
        for field, value in [('index', 9), ('slot', 2), ('raw_ui_hex', 'bb'), ('saved_coefficients_hex', '33' * 844)]:
            wrong = dict(target, **{field: value})
            with patch('server.worker', return_value=wrong):
                with self.assertRaisesRegex(ValueError, 'different preset'):
                    self.app.perform('/api/connect', dict(transport='dongle'))
            self.assertEqual(self.app._session(), before)
        with patch('server.worker', side_effect=OSError('Disconnected')):
            with self.assertRaises(OSError):
                self.app.perform('/api/connect', dict(transport='dongle'))
        self.assertEqual(self.app._session(), before)

    def test_uncertain_storage_blocks_handoff_without_reading(self):
        self.setup_saved_session()
        self.app.storage_state = 'uncertain'
        with patch('server.worker') as worker:
            with self.assertRaisesRegex(ValueError, 'uncertain'):
                self.app.perform('/api/connect', dict(transport='dongle'))
            worker.assert_not_called()

    def test_handoff_disk_failure_leaves_old_session_intact(self):
        target = self.setup_saved_session()
        before = copy.deepcopy(self.app._session())
        with patch('server.worker', return_value=target), patch.object(self.app, '_persist', side_effect=OSError('Disk full')):
            with self.assertRaises(OSError):
                self.app.perform('/api/connect', dict(transport='dongle'))
        self.assertEqual(self.app._session(), before)


if __name__ == '__main__':
    unittest.main()
