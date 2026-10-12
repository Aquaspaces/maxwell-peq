"""Persistent recovery tests; workers are mocked and no HID devices are opened."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import server


class PersistentSessionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.path = Path(self.temporary.name) / 'session.json'
        self.original = '11' * 844
        self.modified = '22' * 844
        self.snapshot = dict(device=dict(path='test-headset', transport='usb'), index=7, slot=0,
                             raw_ui_hex='aa', saved_coefficients_hex=self.original)
        self.app = server.App(True, self.path)
        self.app.backup = dict(state=copy.deepcopy(self.snapshot))
        self.app.backup_path = str(self.path.parent / 'original.json')
        self.app.expected = copy.deepcopy(self.snapshot)
        self.preset = dict(filters=[dict(type='peak', frequency=1000, gain=-3, q=1)], preamp='auto')

    def saved(self, blob=None):
        state = copy.deepcopy(self.snapshot)
        state['saved_coefficients_hex'] = blob or self.modified
        return dict(saved_verified=True, live_applied=True, state=state)

    def test_save_journal_precedes_write_and_original_survives_restart(self):
        def worker(script, request):
            if script == 'peq.py':
                return dict(blob_hex=self.modified)
            pending = json.loads(self.path.read_text())
            self.assertEqual(pending['storage_state'], 'uncertain')
            self.assertEqual(pending['pending']['blob_hex'], self.modified)
            self.assertEqual(pending['backup']['state'], self.snapshot)
            self.assertEqual(request['operation'], 'save')
            return self.saved()
        with patch('server.worker', side_effect=worker):
            self.app.perform('/api/save', self.preset)
        restored = server.App(True, self.path)
        self.assertTrue(restored.session_recovered)
        self.assertEqual(restored.storage_state, 'saved')
        self.assertEqual(restored.expected['saved_coefficients_hex'], self.modified)
        self.assertEqual(restored.backup['state'], self.snapshot)
        self.assertEqual(restored.last_profile, self.preset)
        self.assertIsNone(restored.pending)
        self.assertEqual(len(list(self.path.parent.glob('*-save.json'))), 1)

    def test_repeat_save_uses_current_expected_and_keeps_original(self):
        with patch('server.worker', side_effect=[dict(blob_hex=self.modified), self.saved()]):
            self.app.perform('/api/save', self.preset)
        with patch('server.worker', side_effect=[dict(blob_hex=self.modified), self.saved()]) as worker:
            self.app.perform('/api/save', self.preset)
            self.assertEqual(worker.call_args.args[1]['expected']['saved_coefficients_hex'], self.modified)
        self.assertEqual(self.app.backup['state']['saved_coefficients_hex'], self.original)

    def test_timeout_recovers_and_compensating_restore_needs_no_compiler(self):
        with patch('server.worker', side_effect=[dict(blob_hex=self.modified), RuntimeError('timeout')]):
            with self.assertRaisesRegex(RuntimeError, 'timeout'):
                self.app.perform('/api/save', self.preset)
        recovered = server.App(True, self.path)
        self.assertEqual(recovered.storage_state, 'uncertain')
        self.assertEqual(recovered.live_state, 'uncertain')
        with patch('server.worker') as worker:
            for endpoint in ('/api/apply', '/api/save', '/api/read_device', '/api/restore'):
                with self.assertRaises(ValueError):
                    recovered.perform(endpoint, {})
            worker.assert_not_called()
        with patch('server.peq.verified_dll', side_effect=ValueError('missing DLL')):
            with patch('server.worker', return_value=self.saved(self.original)) as worker:
                recovered.perform('/api/restore_saved', {})
                self.assertEqual(worker.call_args.args[0], 'device_worker.py')
                self.assertEqual(worker.call_args.args[1]['blob_hex'], self.original)
                self.assertEqual(worker.call_args.args[1]['operation'], 'restore_saved')
        self.assertEqual(recovered.storage_state, 'unchanged')
        self.assertEqual(recovered.live_state, 'unchanged')
        self.assertIsNotNone(recovered.backup)

    def test_readback_mismatch_is_uncertain_and_keeps_backup(self):
        error = server.WorkerError(dict(error='Readback mismatch', write_started=True,
                                        persistent_write_started=True, saved_verified=False))
        with patch('server.worker', side_effect=[dict(blob_hex=self.modified), error]):
            with self.assertRaises(server.WorkerError):
                self.app.perform('/api/save', self.preset)
        self.assertEqual(self.app.storage_state, 'uncertain')
        self.assertEqual(self.app.expected, self.snapshot)
        self.assertEqual(server.App(True, self.path).backup['state'], self.snapshot)

    def test_verified_storage_survives_live_failure(self):
        error = server.WorkerError(dict(error='Live ACK timeout', write_started=True,
                                        persistent_write_started=True, saved_verified=True,
                                        state=self.saved()['state']))
        with patch('server.worker', side_effect=[dict(blob_hex=self.modified), error]):
            with self.assertRaisesRegex(server.WorkerError, 'Saved coefficients were read back'):
                self.app.perform('/api/save', self.preset)
        recovered = server.App(True, self.path)
        self.assertEqual(recovered.storage_state, 'saved')
        self.assertEqual(recovered.live_state, 'uncertain')
        self.assertEqual(recovered.expected['saved_coefficients_hex'], self.modified)

    def test_preflight_failure_preserves_prior_states(self):
        self.app.storage_state, self.app.live_state = 'saved', 'applied'
        error = server.WorkerError(dict(error='Disconnected', write_started=False,
                                        persistent_write_started=False, state_changed=True))
        with patch('server.worker', side_effect=error):
            with self.assertRaises(server.WorkerError):
                self.app.perform('/api/restore_saved', {})
        self.assertEqual(self.app.storage_state, 'saved')
        self.assertEqual(self.app.live_state, 'applied')
        self.assertIsNotNone(self.app.backup)

    def test_journal_failure_prevents_device_write(self):
        with patch.object(self.app, '_persist', side_effect=OSError('Disk full')):
            with patch('server.worker', return_value=dict(blob_hex=self.modified)) as worker:
                with self.assertRaises(OSError):
                    self.app.perform('/api/save', self.preset)
                self.assertEqual(worker.call_count, 1)
                self.assertEqual(worker.call_args.args[0], 'peq.py')
        self.assertEqual(self.app.storage_state, 'unchanged')

    def test_dongle_route_is_explicit_and_preserved(self):
        state = copy.deepcopy(self.snapshot)
        state['device']['transport'] = 'dongle'
        with patch('server.worker', return_value=state) as worker:
            self.app.perform('/api/read_device', dict(transport='dongle'))
            self.assertEqual(worker.call_args.args[1], dict(operation='read', transport='dongle'))
        recovered = server.App(True, self.path)
        with patch('server.worker', return_value={}) as worker:
            recovered.perform('/api/restore', {})
            self.assertEqual(worker.call_args.args[1]['transport'], 'dongle')

    def test_corrupt_recovery_file_fails_closed(self):
        self.path.write_text(json.dumps(dict(version=1, live_state='uncertain')), encoding='utf-8')
        with self.assertRaisesRegex(ValueError, 'Invalid recovery'):
            server.App(True, self.path)

    def test_offline_mode_blocks_storage_writes(self):
        self.app.enable_live = False
        with patch('server.worker') as worker:
            for endpoint in ('/api/save', '/api/restore_saved'):
                with self.assertRaises(ValueError):
                    self.app.perform(endpoint, {})
            worker.assert_not_called()


if __name__ == '__main__':
    unittest.main()
