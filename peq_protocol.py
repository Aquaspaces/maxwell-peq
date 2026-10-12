"""Offline-derived Maxwell 1 RACE framing. This module never opens hardware.

Evidence: PROOF.md and the local analysis paths listed there.
This is a proof of concept; an ACK establishes command acceptance, not acoustics.
"""
from dataclasses import dataclass
import struct

MAX_PACKET = 1024
CHUNK = 59
REPORT_BYTES = 62


class ProtocolError(ValueError):
    pass


def _packet(command: int, payload: bytes) -> bytes:
    packet = b"\x05\x5a" + struct.pack("<HH", len(payload) + 2, command) + payload
    if len(packet) > MAX_PACKET:
        raise ProtocolError("Packet exceeds the vendor receive buffer")
    return packet


def read_index_command() -> bytes:
    return bytes.fromhex("05 5a 04 00 01 09 00 00")


def read_ui_command(slot: int) -> bytes:
    if type(slot) is not int or not 0 <= slot <= 3:
        raise ProtocolError("Custom preset storage slot must be 0..3")
    return _packet(0x0A0C, struct.pack("<HH", 0xEF00 + slot, 1000))


def read_coefficients_command(slot: int) -> bytes:
    read_ui_command(slot)
    return _packet(0x0A0C, struct.pack('<HH', 0xE42C + slot, 1000))


def live_command(coefficients: bytes) -> bytes:
    if not isinstance(coefficients, bytes) or not coefficients:
        raise ProtocolError("Coefficient blob must be nonempty bytes")
    return _packet(0x0E03, b"\x00" + coefficients)


def storage_space_command() -> bytes:
    # Firmware query only: returns whether an 844-byte NVDM value fits.
    return _packet(0x0A03, struct.pack('<H', 844))


def save_coefficients_command(slot: int, coefficients: bytes) -> bytes:
    read_ui_command(slot)
    if not isinstance(coefficients, bytes) or len(coefficients) != 844:
        raise ProtocolError('Saved coefficients must be the 844-byte four-rate format')
    return _packet(0x0A0D, struct.pack('<H', 0xE42C + slot) + coefficients)


def check_packet(packet: bytes, kind: int) -> tuple[int, bytes]:
    if len(packet) < 6 or len(packet) > MAX_PACKET or packet[:2] != bytes((5, kind)):
        raise ProtocolError("Invalid RACE header")
    length, command = struct.unpack_from("<HH", packet, 2)
    if length < 2 or length + 4 != len(packet):
        raise ProtocolError("RACE length does not match complete packet")
    return command, packet[6:]


def validate_request(packet: bytes, *, allow_live: bool = False, allow_persistent: bool = False) -> None:
    """Only reads, live EQ, and opt-in writes to the four coefficient keys."""
    command, payload = check_packet(packet, 0x5A)
    if packet == read_index_command():
        return
    if command == 0x0A0C and payload in [struct.pack("<HH", base + i, 1000) for base in (0xEF00, 0xE42C) for i in range(4)]:
        return
    if allow_live and command == 0x0E03 and len(payload) > 1 and payload[0] == 0:
        return
    if packet == storage_space_command():
        return
    if allow_persistent and command == 0x0A0D and len(payload) == 846 and 0xE42C <= struct.unpack_from('<H', payload)[0] <= 0xE42F:
        return
    raise ProtocolError("Command is outside the EQ allowlist")


def parse_coefficients(packet: bytes, slot: int) -> bytes:
    read_ui_command(slot)
    command, payload = check_packet(packet, 0x5B)
    if command != 0x0A0C or len(payload) < 5 or payload[0] != 0 or payload[1:3] != struct.pack('<H', 0xE42C + slot):
        raise ProtocolError('Unexpected saved coefficient response')
    raw = packet[11:]
    if struct.unpack_from('<H', payload, 3)[0] != len(raw):
        raise ProtocolError('Saved coefficient length does not match data')
    return raw


def output_reports(packet: bytes, *, dongle: bool = False, report_size: int = REPORT_BYTES,
                   allow_live: bool = False, allow_persistent: bool = False) -> list[bytes]:
    validate_request(packet, allow_live=allow_live, allow_persistent=allow_persistent)
    if not REPORT_BYTES <= report_size <= 1024:
        raise ProtocolError("Unsupported HID report size")
    return [bytes((6, len(chunk), 0x80 if dongle else 0)) + chunk +
            bytes(report_size - 3 - len(chunk))
            for start in range(0, len(packet), CHUNK)
            for chunk in [packet[start:start + CHUNK]]]


class InputStream:
    """Reassemble input reports; retain multiple complete messages in one report."""
    def __init__(self):
        self.pending = bytearray()

    def feed(self, report: bytes) -> list[bytes]:
        if len(report) < REPORT_BYTES or report[0] != 7 or report[1] > CHUNK:
            raise ProtocolError("Invalid HID input report")
        # Byte2 is metadata, not the upper byte of a length.
        self.pending.extend(report[3:3 + report[1]])
        messages = []
        while len(self.pending) >= 4:
            if self.pending[0] != 5:
                raise ProtocolError("RACE stream lost its packet boundary")
            size = struct.unpack_from("<H", self.pending, 2)[0] + 4
            if not 6 <= size <= MAX_PACKET:
                raise ProtocolError("Invalid RACE stream length")
            if len(self.pending) < size:
                break
            messages.append(bytes(self.pending[:size]))
            del self.pending[:size]
        return messages


def parse_index(packet: bytes) -> int:
    command, payload = check_packet(packet, 0x5B)
    if command != 0x0901 or len(payload) != 4 or payload[:2] != b"\0\0":
        raise ProtocolError("Unexpected current-preset reply")
    # App does not interpret payload[2]; preserve full raw reply in backups.
    return payload[3]


def parse_live_ack(packet: bytes) -> None:
    command, payload = check_packet(packet, 0x5B)
    if command != 0x0E03 or len(payload) != 1:
        raise ProtocolError("Unexpected live-EQ acknowledgement")
    if payload[0] != 0:
        raise ProtocolError(f"Headset rejected live EQ (status {payload[0]})")


def parse_storage_space(packet: bytes) -> bool:
    command, payload = check_packet(packet, 0x5B)
    if command != 0x0A03 or len(payload) != 1 or payload[0] not in (0, 1):
        raise ProtocolError('Unexpected storage-space response')
    return bool(payload[0])  # 1 means enough free space; this is not a status byte.


def parse_save_ack(packet: bytes, slot: int) -> None:
    read_ui_command(slot)
    command, payload = check_packet(packet, 0x5B)
    if command != 0x0A0D or len(payload) != 3 or payload[1:] != struct.pack('<H', 0xE42C + slot):
        raise ProtocolError('Unexpected coefficient-save acknowledgement')
    if payload[0] != 0:
        raise ProtocolError(f'Headset rejected coefficient save (status {payload[0]})')


@dataclass(frozen=True)
class UiBand:
    tag: int
    frequency_hundredths: int
    gain_hundredths: int
    width_hundredths: int
    unknown: int


@dataclass(frozen=True)
class UiPreset:
    slot: int
    header: bytes
    bands: tuple[UiBand, ...]
    raw_payload: bytes
    raw_response: bytes


def parse_ui(packet: bytes, slot: int) -> UiPreset:
    read_ui_command(slot)
    command, payload = check_packet(packet, 0x5B)
    length = len(packet) - 4
    if command != 0x0A0C or not 12 <= length <= 192 or len(payload) < 10:
        raise ProtocolError("Unexpected preset UI reply")
    if payload[0] != 0 or payload[1:3] != struct.pack("<H", 0xEF00 + slot):
        raise ProtocolError("Preset UI reply has failure status or wrong key")
    raw = packet[11:]
    if struct.unpack_from('<H', payload, 3)[0] != len(raw):
        raise ProtocolError('Preset UI stored length does not match data')
    if len(raw) < 5 or (len(raw) - 5) % 18:
        raise ProtocolError("Preset UI record length is invalid")
    bands = tuple(UiBand(*struct.unpack_from("<Hiiii", raw, offset))
                  for offset in range(5, len(raw), 18))
    # Header interpretation is deliberately deferred: app decoder/encoder disagree.
    return UiPreset(slot, raw[:5], bands, raw, packet)
