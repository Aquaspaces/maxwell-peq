"""Windows Maxwell PS direct-cable transport. No hardware access at import.

Only opening an instance enumerates HID devices. All writes use the exact
read/live-only command allowlist. HID native calls are synchronous: transaction
timeouts bound polling but cannot interrupt a stalled HidD_GetInputReport call.
Call from a dedicated worker process if the UI needs a hard wall-clock deadline.
"""
import ctypes as C
from ctypes import wintypes as W
import os
import threading
import time
from peq_protocol import (InputStream, ProtocolError, check_packet, output_reports,
                          validate_request, parse_index, parse_ui, parse_live_ack,
                          read_index_command, read_ui_command, live_command, parse_coefficients,
                          read_coefficients_command)


class GUID(C.Structure):
    _fields_ = [("a", W.DWORD), ("b", W.WORD), ("c", W.WORD), ("d", C.c_ubyte * 8)]


class InterfaceData(C.Structure):
    _fields_ = [("size", W.DWORD), ("guid", GUID), ("flags", W.DWORD), ("reserved", C.c_size_t)]


class Attributes(C.Structure):
    _fields_ = [("size", W.ULONG), ("vid", W.USHORT), ("pid", W.USHORT), ("version", W.USHORT)]


class Caps(C.Structure):
    _fields_ = [("usage", W.USHORT), ("page", W.USHORT), ("input", W.USHORT),
                ("output", W.USHORT), ("feature", W.USHORT), ("reserved", W.USHORT * 17),
                ("remaining", W.USHORT * 10)]


def _bind(dll, name, result, *args):
    function = getattr(dll, name)
    function.restype, function.argtypes = result, args
    return function


class MaxwellPsHid:
    """Use as a context manager. Session defaults to reads until allow_live=True."""
    def __init__(self, *, allow_live=False):
        if os.name != "nt":
            raise OSError("This transport requires Windows")
        self.allow_live = allow_live
        self._lock = threading.Lock()
        self.handle = None
        k = C.WinDLL("kernel32", use_last_error=True)
        h = C.WinDLL("hid", use_last_error=True)
        s = C.WinDLL("setupapi", use_last_error=True)
        ptr = C.c_void_p
        self.create = _bind(k, "CreateFileW", W.HANDLE, W.LPCWSTR, W.DWORD, W.DWORD,
                            ptr, W.DWORD, W.DWORD, W.HANDLE)
        self.close_handle = _bind(k, "CloseHandle", W.BOOL, W.HANDLE)
        self.write = _bind(k, "WriteFile", W.BOOL, W.HANDLE, ptr, W.DWORD, C.POINTER(W.DWORD), ptr)
        self.read = _bind(h, "HidD_GetInputReport", C.c_ubyte, W.HANDLE, ptr, W.ULONG)
        self.attributes = _bind(h, "HidD_GetAttributes", C.c_ubyte, W.HANDLE, C.POINTER(Attributes))
        self.preparsed = _bind(h, "HidD_GetPreparsedData", C.c_ubyte, W.HANDLE, C.POINTER(ptr))
        self.free_preparsed = _bind(h, "HidD_FreePreparsedData", C.c_ubyte, ptr)
        self.caps = _bind(h, "HidP_GetCaps", W.LONG, ptr, C.POINTER(Caps))
        get_guid = _bind(h, "HidD_GetHidGuid", None, C.POINTER(GUID))
        get_set = _bind(s, "SetupDiGetClassDevsW", W.HANDLE, C.POINTER(GUID), W.LPCWSTR, W.HWND, W.DWORD)
        enum = _bind(s, "SetupDiEnumDeviceInterfaces", W.BOOL, W.HANDLE, ptr, C.POINTER(GUID), W.DWORD, C.POINTER(InterfaceData))
        detail = _bind(s, "SetupDiGetDeviceInterfaceDetailW", W.BOOL, W.HANDLE,
                       C.POINTER(InterfaceData), ptr, W.DWORD, C.POINTER(W.DWORD), ptr)
        destroy = _bind(s, "SetupDiDestroyDeviceInfoList", W.BOOL, W.HANDLE)
        invalid = C.c_void_p(-1).value
        guid = GUID()
        get_guid(C.byref(guid))
        devices = get_set(C.byref(guid), None, None, 0x12)
        if devices == invalid:
            raise C.WinError(C.get_last_error())
        candidates = []
        try:
            index = 0
            while True:
                entry = InterfaceData()
                entry.size = C.sizeof(entry)
                if not enum(devices, None, C.byref(guid), index, C.byref(entry)):
                    if C.get_last_error() == 259:
                        break
                    raise C.WinError(C.get_last_error())
                index += 1
                need = W.DWORD()
                detail(devices, C.byref(entry), None, 0, C.byref(need), None)
                if need.value < 8:
                    continue
                buffer = C.create_string_buffer(need.value)
                C.cast(buffer, C.POINTER(W.DWORD))[0] = 8 if C.sizeof(ptr) == 8 else 6
                if not detail(devices, C.byref(entry), buffer, need.value, C.byref(need), None):
                    continue
                path = C.wstring_at(C.addressof(buffer) + 4)
                probe = self.create(path, 0, 3, None, 3, 0, None)
                if probe == invalid:
                    continue
                try:
                    attr = Attributes()
                    attr.size = C.sizeof(attr)
                    if not self.attributes(probe, C.byref(attr)) or (attr.vid, attr.pid) != (0x3329, 0x4B1A):
                        continue
                    prep = ptr()
                    if not self.preparsed(probe, C.byref(prep)):
                        continue
                    try:
                        caps = Caps()
                        status = self.caps(prep, C.byref(caps))
                    finally:
                        self.free_preparsed(prep)
                    if status >= 0 and caps.page == 0xFF13 and 62 <= caps.input <= 1024 and 62 <= caps.output <= 1024:
                        candidates.append((path, caps.input, caps.output, attr.version))
                finally:
                    self.close_handle(probe)
        finally:
            destroy(devices)
        if len(candidates) != 1:
            raise OSError(f"Expected one original Maxwell PlayStation USB-C interface, found {len(candidates)}")
        self.path, self.input_size, self.output_size, self.device_version = candidates[0]
        self.handle = self.create(self.path, 0xC0000000, 3, None, 3, 0, None)
        if self.handle == invalid:
            self.handle = None
            raise C.WinError(C.get_last_error())

    def close(self):
        if self.handle is not None:
            self.close_handle(self.handle)
            self.handle = None

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()

    def read_state(self):
        """Read-only snapshot. UI data is available only for current custom slots."""
        raw_index = self.transact(read_index_command())
        index = parse_index(raw_index)
        state = {"device": {"vid": 0x3329, "pid": 0x4B1A, "usage_page": 0xFF13,
                            "path": self.path, "bcd_device": self.device_version,
                            "input_report_bytes": self.input_size, "output_report_bytes": self.output_size},
                 "firmware": None, "firmware_note": "Firmware command is outside this minimal allowlist",
                 "index": index, "slot": index - 7 if 7 <= index <= 10 else None,
                 "raw_index_hex": raw_index.hex(), "bands": None, "raw_ui_hex": None}
        if state["slot"] is not None:
            preset = parse_ui(self.transact(read_ui_command(state["slot"])), state["slot"])
            state["raw_ui_hex"] = preset.raw_payload.hex()
            state["raw_ui_response_hex"] = preset.raw_response.hex()
            state["ui_header_hex"] = preset.header.hex()
            state["bands"] = [{"tag": band.tag, "frequency_hundredths": band.frequency_hundredths,
                               "gain_hundredths": band.gain_hundredths, "width_hundredths": band.width_hundredths,
                               "unknown": band.unknown} for band in preset.bands]
            coefficient_reply = self.transact(read_coefficients_command(state['slot']))
            state['saved_coefficients_hex'] = parse_coefficients(coefficient_reply, state['slot']).hex()
            state['raw_coefficient_response_hex'] = coefficient_reply.hex()
            if parse_index(self.transact(read_index_command())) != index:
                raise ProtocolError("Preset changed while taking the backup; retry read-only snapshot")
        return state

    def apply_live(self, coefficients: bytes):
        """Send one live update; callers enforce a prior backup and filter validation."""
        ack = self.transact(live_command(coefficients))
        return {"accepted": True, "ack_hex": ack.hex(), "persistence_write_sent": False}

    def _input(self):
        report = (C.c_ubyte * self.input_size)()
        report[0] = 7
        if not self.read(self.handle, report, len(report)):
            raise C.WinError(C.get_last_error())
        return bytes(report)

    def transact(self, packet: bytes, *, timeout=3.0) -> bytes:
        validate_request(packet, allow_live=self.allow_live)
        if not self.handle:
            raise OSError("HID session is closed")
        if not 0.1 <= timeout <= 30:
            raise ValueError("Timeout must be 0.1..30 seconds")
        command, _ = check_packet(packet, 0x5A)
        with self._lock:
            # Empty old responses before writing. Failure to empty aborts the write.
            for _ in range(100):
                report = self._input()
                if report[0] != 7 or report[1] > 59:
                    raise ProtocolError("Malformed report while draining input")
                if report[1] == 0:
                    break
                time.sleep(0.01)
            else:
                raise ProtocolError("Input queue did not drain; no command sent")
            for report in output_reports(packet, report_size=self.output_size, allow_live=self.allow_live):
                raw = (C.c_ubyte * len(report)).from_buffer_copy(report)
                count = W.DWORD()
                if not self.write(self.handle, raw, len(raw), C.byref(count), None) or count.value != len(raw):
                    raise OSError("Incomplete HID write; live state may be uncertain")
            stream = InputStream()
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                time.sleep(0.01)
                for message in stream.feed(self._input()):
                    if message[:2] == b"\x05\x5b" and check_packet(message, 0x5B)[0] == command:
                        if command == 0x0E03:
                            parse_live_ack(message)
                        elif command == 0x0901:
                            parse_index(message)
                        elif packet[7] == 0xEF:
                            parse_ui(message, packet[6])
                        else:
                            parse_coefficients(message, packet[6] - 0x2C)
                        return message
            raise TimeoutError("No matching command response; live state may be uncertain")
