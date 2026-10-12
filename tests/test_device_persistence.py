import copy
import struct
import unittest
from unittest.mock import MagicMock, patch

import device_worker as worker
from extended_filters import encode_bank
from peq import RATES


def blob(preamp):
    raw = bytearray(struct.pack('<I', 4))
    for rate_id, rate in RATES:
        raw += struct.pack('<HH103H', rate_id, 103, *encode_bank([], preamp, rate))
    return raw.hex()


class DevicePersistenceTests(unittest.TestCase):
    def setUp(self):
        self.original, self.desired = blob(0), blob(-3)
        self.state = dict(device={'path':'test-dongle', 'transport':'dongle'}, index=10, slot=3,
                          raw_ui_hex='00', saved_coefficients_hex=self.original)
        self.saved = dict(self.state, saved_coefficients_hex=self.desired)
        self.device = MagicMock()
        self.device.__enter__.return_value = self.device
        self.device.has_storage_space.return_value = True
        self.device.save_coefficients.return_value = {'saved_verified': True}
        self.device.apply_live.return_value = {'accepted': True, 'ack_hex':'055b0300030e00'}

    def request(self, operation='save'):
        return dict(operation=operation, transport='dongle', expected=copy.deepcopy(self.state), blob_hex=self.desired)

    def test_save_checks_snapshot_then_verifies_then_applies(self):
        self.device.read_state.side_effect = [self.state, self.saved]
        with patch('device_worker.MaxwellPsHid', return_value=self.device) as factory:
            result = worker.execute(self.request())
        factory.assert_called_once_with(allow_live=True, allow_persistent=True, transport='dongle')
        self.device.save_coefficients.assert_called_once_with(3, bytes.fromhex(self.desired))
        self.assertTrue(result['saved_verified'] and result['live_applied'])
        self.assertEqual(result['state']['saved_coefficients_hex'], self.desired)

    def test_no_space_sends_no_profile_or_live_write(self):
        self.device.read_state.return_value = self.state
        self.device.has_storage_space.return_value = False
        with patch('device_worker.MaxwellPsHid', return_value=self.device), self.assertRaisesRegex(ValueError, 'insufficient'):
            worker.execute(self.request())
        self.device.save_coefficients.assert_not_called()
        self.device.apply_live.assert_not_called()
        self.assertFalse(worker.WRITE_STARTED)

    def test_readback_failure_keeps_persistent_uncertainty(self):
        self.device.read_state.return_value = self.state
        self.device.save_coefficients.side_effect = ValueError('Readback mismatch')
        with patch('device_worker.MaxwellPsHid', return_value=self.device), self.assertRaises(ValueError):
            worker.execute(self.request())
        self.assertTrue(worker.PERSISTENT_WRITE_STARTED)
        self.assertFalse(worker.SAVED_VERIFIED)
        self.device.apply_live.assert_not_called()

    def test_live_failure_preserves_verified_saved_state(self):
        self.device.read_state.side_effect = [self.state, self.saved]
        self.device.apply_live.side_effect = TimeoutError('No live ACK')
        with patch('device_worker.MaxwellPsHid', return_value=self.device), self.assertRaises(TimeoutError):
            worker.execute(self.request())
        self.assertTrue(worker.SAVED_VERIFIED)
        self.assertEqual(worker.LAST_STATE, self.saved)

    def test_restore_can_repair_bad_coefficients_but_not_a_different_slot(self):
        damaged = dict(self.state, saved_coefficients_hex='1122')
        self.device.read_state.side_effect = [damaged, self.saved]
        with patch('device_worker.MaxwellPsHid', return_value=self.device):
            worker.execute(self.request('restore_saved'))
        self.device.reset_mock()
        self.device.read_state.side_effect = [dict(damaged, index=9, slot=2)]
        with patch('device_worker.MaxwellPsHid', return_value=self.device), self.assertRaises(worker.StateChanged):
            worker.execute(self.request('restore_saved'))
        self.device.save_coefficients.assert_not_called()
