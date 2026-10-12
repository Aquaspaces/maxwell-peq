"""Recovery state tests use fake workers only; never open a headset."""
import unittest
from unittest.mock import patch

import server


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.app = server.App(enable_live=True)
        self.snapshot = {'saved_coefficients_hex': '1122', 'slot': 0}
        self.app.backup = {'state': self.snapshot}
        self.app.backup_path = 'test-backup.json'

    def test_changed_preset_before_send_allows_new_backup(self):
        error = server.WorkerError(dict(error='Changed preset', write_started=False, state_changed=True))
        with patch('server.worker', side_effect=[{'blob_hex': '3344'}, error]):
            with self.assertRaises(server.WorkerError):
                self.app.perform('/api/apply', {})
        self.assertEqual(self.app.live_state, 'unchanged')
        self.assertIsNone(self.app.backup)
        self.assertIsNone(self.app.backup_path)

    def test_preflight_disconnect_keeps_existing_live_state(self):
        self.app.live_state = 'applied'
        error = server.WorkerError(dict(error='No headset', write_started=False, state_changed=False))
        with patch('server.worker', side_effect=error):
            with self.assertRaises(server.WorkerError):
                self.app.perform('/api/restore', {})
        self.assertEqual(self.app.live_state, 'applied')
        self.assertIsNotNone(self.app.backup)

    def test_timeout_blocks_apply_but_restore_uses_exact_saved_bytes(self):
        with patch('server.worker', side_effect=[{'blob_hex': '3344'}, RuntimeError('Timeout')]):
            with self.assertRaises(RuntimeError):
                self.app.perform('/api/apply', {})
        self.assertEqual(self.app.live_state, 'uncertain')
        with patch('server.worker') as worker:
            with self.assertRaisesRegex(ValueError, 'Restore'):
                self.app.perform('/api/apply', {})
            worker.assert_not_called()
        with patch('server.worker', return_value={}) as worker:
            self.app.perform('/api/restore', {})
            worker.assert_called_once_with('device_worker.py', {
                'operation': 'apply', 'blob_hex': '1122', 'expected': self.snapshot, 'transport': 'usb'})
        self.assertEqual(self.app.live_state, 'unchanged')

    def test_unacknowledged_write_retains_backup(self):
        error = server.WorkerError(dict(error='Rejected', write_started=True, state_changed=False))
        with patch('server.worker', side_effect=[{'blob_hex': '3344'}, error]):
            with self.assertRaises(server.WorkerError):
                self.app.perform('/api/apply', {})
        self.assertEqual(self.app.live_state, 'uncertain')
        self.assertIsNotNone(self.app.backup)

    def test_compile_failure_does_not_change_device_state(self):
        with patch('server.worker', side_effect=ValueError('Invalid filter')):
            with self.assertRaises(ValueError):
                self.app.perform('/api/apply', {})
        self.assertEqual(self.app.live_state, 'unchanged')

    def test_offline_mode_blocks_device_access(self):
        self.app.enable_live = False
        for path in ('/api/read_device', '/api/apply', '/api/restore'):
            with patch('server.worker') as worker:
                with self.assertRaises(ValueError):
                    self.app.perform(path, {})
                worker.assert_not_called()
