import struct
import unittest
from peq_protocol import *


def response(command, payload):
    return b"\x05\x5b" + struct.pack("<HH", len(payload) + 2, command) + payload


def reports(data):
    return [bytes((7, len(chunk), 0x80)) + chunk + bytes(59 - len(chunk))
            for start in range(0, len(data), 59) for chunk in [data[start:start + 59]]]


class ProtocolTests(unittest.TestCase):
    def test_vendor_request_vectors(self):
        self.assertEqual(read_index_command().hex(), "055a040001090000")
        self.assertEqual(read_ui_command(2).hex(), "055a06000c0a02efe803")
        self.assertEqual(live_command(bytes.fromhex("12345678")).hex(), "055a0700030e0012345678")
        self.assertEqual(output_reports(read_index_command())[0][:11].hex(), "060800055a040001090000")

    def test_fragmentation_and_routing(self):
        packet = live_command(bytes(range(256)) * 3)
        chunks = output_reports(packet, dongle=True, report_size=63, allow_live=True)
        self.assertTrue(all(len(r) == 63 and r[:1] == b"\x06" and r[2] == 0x80 for r in chunks))
        self.assertTrue(all(r[1] == 59 for r in chunks[:-1]))
        self.assertEqual(b"".join(r[3:3 + r[1]] for r in chunks), packet)

    def test_allowlist(self):
        for packet in [response(0x0E03, b"\0"), bytes.fromhex("055a05000009000007"),
                       bytes.fromhex("055a06000c0a30e4e803"), bytes.fromhex("055a04000d0a2ce4")]:
            with self.assertRaises(ProtocolError):
                validate_request(packet, allow_live=True)
        with self.assertRaises(ProtocolError):
            output_reports(live_command(b"\0\0"))
        with self.assertRaises(ProtocolError):
            read_ui_command(4)

    def test_saved_coefficient_read(self):
        packet = read_coefficients_command(0)
        self.assertEqual(packet.hex(), '055a06000c0a2ce4e803')
        validate_request(packet)
        raw = bytes(844)
        reply = response(0x0A0C, bytes.fromhex('002ce44c03') + raw)
        self.assertEqual(parse_coefficients(reply, 0), raw)
        with self.assertRaises(ProtocolError):
            parse_coefficients(reply, 1)
        with self.assertRaises(ProtocolError):
            parse_coefficients(response(0x0A0C, bytes.fromhex('002ce44b03') + raw), 0)

    def test_reassembly_split_and_multiple(self):
        packet = response(0x0A0C, bytes(range(200)))
        ack = response(0x0E03, b"\0")
        stream = InputStream()
        results = []
        for report in reports(packet + ack):
            results.extend(stream.feed(report))
        self.assertEqual(results, [packet, ack])
        self.assertEqual(stream.pending, b"")

    def test_bounds(self):
        with self.assertRaises(ProtocolError):
            live_command(bytes(1024))
        with self.assertRaises(ProtocolError):
            InputStream().feed(bytes((7, 60, 0)) + bytes(59))
        with self.assertRaises(ProtocolError):
            InputStream().feed(reports(bytes.fromhex("055bffff"))[0])
        with self.assertRaises(ProtocolError):
            check_packet(response(0x0E03, b"\0") + b"\0", 0x5B)

    def test_ack_failures(self):
        parse_live_ack(response(0x0E03, b"\0"))
        for packet in [response(0x0E03, b"\1"), response(0x0A0D, b"\0"),
                       response(0x0E03, b""), response(0x0E03, b"\0\0")]:
            with self.assertRaises(ProtocolError):
                parse_live_ack(packet)

    def test_index(self):
        self.assertEqual(parse_index(response(0x0901, b"\0\0\1\x07")), 7)
        with self.assertRaises(ProtocolError):
            parse_index(response(0x0901, b"\x29\0\1\x07"))

    def test_ui_payload_preserves_unknowns(self):
        band = struct.pack("<Hiiii", 0x102, 100000, -600, 12500, 77)
        raw = bytes.fromhex("0102030405") + band * 10
        packet = response(0x0A0C, b"\0\x02\xef" + struct.pack("<H", len(raw)) + raw)
        preset = parse_ui(packet, 2)
        self.assertEqual(preset.raw_response, packet)
        self.assertEqual(preset.header, bytes.fromhex("0102030405"))
        self.assertEqual(preset.bands[0], UiBand(0x102, 100000, -600, 12500, 77))
        self.assertEqual(len(preset.bands), 10)
        with self.assertRaises(ProtocolError):
            parse_ui(packet, 1)
        with self.assertRaises(ProtocolError):
            parse_ui(response(0x0A0C, b'\0\x02\xef\0\0' + raw), 2)


if __name__ == "__main__":
    unittest.main()
