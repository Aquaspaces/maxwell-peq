"""Read-only connection handoffs; workers are mocked and no HID devices are opened."""
import copy
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import server
from server import App


class ConnectionHandoffTests(unittest.TestCase):
    def setUp(self):
        audeze = patch.object(server.AudezeWatch, 'running', return_value=[])  # the real check reads this PC's processes
        audeze.start()
        self.addCleanup(audeze.stop)
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.session = Path(self.directory.name) / 'session.json'
        self.app = App(True, self.session)

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
        restored = App(True, self.session)
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
