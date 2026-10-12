import struct
import unittest

from peq_protocol import (ProtocolError, output_reports, parse_save_ack, parse_storage_space,
                          save_coefficients_command, storage_space_command, validate_request)


def reply(command, payload):
    return bytes.fromhex('055b') + struct.pack('<HH', len(payload) + 2, command) + payload


class StorageProtocolTests(unittest.TestCase):
    def test_save_vector_route_and_permission(self):
        packet = save_coefficients_command(3, bytes(844))
        self.assertEqual(packet[:8].hex(), '055a50030d0a2fe4')
        self.assertEqual(len(packet), 852)
        with self.assertRaises(ProtocolError):
            validate_request(packet, allow_live=True)
        validate_request(packet, allow_persistent=True)
        chunks = output_reports(packet, dongle=True, allow_persistent=True)
        self.assertTrue(all(r[2] == 0x80 and len(r) == 62 for r in chunks))
        self.assertEqual(b''.join(r[3:3+r[1]] for r in chunks), packet)
        wrong_key = packet[:6] + bytes.fromhex('00ef') + packet[8:]
        with self.assertRaises(ProtocolError):
            validate_request(wrong_key, allow_persistent=True)
        with self.assertRaises(ProtocolError):
            save_coefficients_command(4, bytes(844))
        with self.assertRaises(ProtocolError):
            save_coefficients_command(0, bytes(843))

    def test_capacity_is_a_boolean_not_zero_success_status(self):
        self.assertEqual(storage_space_command().hex(), '055a0400030a4c03')
        validate_request(storage_space_command())
        self.assertTrue(parse_storage_space(reply(0x0a03, b'\1')))
        self.assertFalse(parse_storage_space(reply(0x0a03, b'\0')))
        with self.assertRaises(ProtocolError):
            parse_storage_space(reply(0x0a03, b'\2'))

    def test_ack_requires_success_and_the_same_key(self):
        parse_save_ack(bytes.fromhex('055b05000d0a002fe4'), 3)
        for packet in (reply(0x0a0d, bytes.fromhex('012fe4')),
                       reply(0x0a0d, bytes.fromhex('002ee4')),
                       reply(0x0a0d, bytes.fromhex('002fe400'))):
            with self.assertRaises(ProtocolError):
                parse_save_ack(packet, 3)
