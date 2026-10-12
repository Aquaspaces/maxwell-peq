"""Queue settling checks without opening HID devices."""
import unittest
from unittest.mock import Mock, patch

from peq_protocol import ProtocolError
from win_hid import MaxwellPsHid


class InputDrainTests(unittest.TestCase):
    def setUp(self):
        self.device = object.__new__(MaxwellPsHid)
        self.empty = bytes((7, 0, 0)) + bytes(59)
        self.fragment = bytes((7, 1, 0, 42)) + bytes(58)

    @patch('win_hid.time.sleep')
    def test_delayed_fragment_restarts_quiet_period(self, sleep):
        reports = [self.empty, self.fragment] + [self.empty] * 5 + [self.fragment] + [self.empty] * 6
        self.device._input = Mock(side_effect=reports)
        self.device._drain()
        self.assertEqual(self.device._input.call_count, len(reports))

    @patch('win_hid.time.sleep')
    def test_continuous_input_aborts_with_bounded_reads(self, sleep):
        self.device._input = Mock(return_value=self.fragment)
        with self.assertRaisesRegex(ProtocolError, 'no command sent'):
            self.device._drain()
        self.assertEqual(self.device._input.call_count, 100)

    def test_malformed_report_aborts(self):
        self.device._input = Mock(return_value=bytes((7, 60, 0)) + bytes(59))
        with self.assertRaisesRegex(ProtocolError, 'Malformed'):
            self.device._drain()


if __name__ == '__main__':
    unittest.main()
