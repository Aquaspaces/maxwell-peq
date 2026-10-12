"""Exclusive-open contract tests; no hardware access."""
import ctypes
import unittest
from unittest.mock import Mock, patch

from win_hid import MaxwellPsHid


class DeviceAccessTests(unittest.TestCase):
    def test_control_handle_is_exclusive(self):
        device = MaxwellPsHid.__new__(MaxwellPsHid)
        device.path = 'test-hid-path'
        device.create = Mock(return_value=1234)
        device._open_control()
        device.create.assert_called_once_with('test-hid-path', 0xC0000000, 0, None, 3, 0, None)
        self.assertEqual(device.handle, 1234)

    def test_busy_interface_explains_recovery_without_shared_retry(self):
        device = MaxwellPsHid.__new__(MaxwellPsHid)
        device.path = 'test-hid-path'
        device.create = Mock(return_value=ctypes.c_void_p(-1).value)
        with patch('win_hid.C.get_last_error', return_value=32, create=True):
            with self.assertRaisesRegex(OSError, 'Fully quit Audeze HQ.*No EQ was sent'):
                device._open_control()
        self.assertIsNone(device.handle)
        self.assertEqual(device.create.call_count, 1)
