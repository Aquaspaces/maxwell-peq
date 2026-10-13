"""Offline Maxwell EQ compiler. No device I/O."""
from __future__ import annotations

import cmath
import ctypes as C
import hashlib
import json
import math
import os
from pathlib import Path
import struct

from extended_filters import rbj_biquad, filter_headroom_db, encode_bank

DLL_SHA256 = '322ac1149eeba9ed55f2466ba706edc92dcdb333cf574a6466abf88e4b3d50be'
DLL_NAME = 'AirohaPeqLibrary.dll'
# The DLL is never redistributed: use the copy the Audeze app installs, or the inspected extract.
DLL_CANDIDATES = (
    Path(os.environ.get('ProgramFiles', 'C:/Program Files')) / 'Audeze' / 'AudezeApp' / DLL_NAME,
    Path('X:/audeze-re/extract/audeze-app') / DLL_NAME,
)
RATES = ((1, 44100), (2, 48000), (5, 88200), (6, 96000))
MAX_BANDS = 10
FILTER_TYPES = ('peak', 'low_shelf', 'high_shelf', 'low_pass', 'high_pass')
SHELF_Q = math.sqrt(0.5)


def dll_path():
    if os.environ.get('MAXWELL_PEQ_DLL'):
        return Path(os.environ['MAXWELL_PEQ_DLL']).resolve()
    found = [path for path in DLL_CANDIDATES if path.is_file()]
    return (found or [DLL_CANDIDATES[0]])[0].resolve()


def verified_dll():
    path = dll_path()
    if not path.is_file():
        raise ValueError('AirohaPeqLibrary.dll was not found. Install the Audeze app, or set MAXWELL_PEQ_DLL to the inspected DLL.')
    if hashlib.sha256(path.read_bytes()).hexdigest() != DLL_SHA256:
        raise ValueError('The PEQ DLL differs from the inspected version; compilation is disabled.')
    return path


def number(value, name, low, high):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f'{name} must be a number.')
    value = float(value)
    if not math.isfinite(value) or not low <= value <= high:
        raise ValueError(f'{name} must be between {low:g} and {high:g}.')
    return value


def validate(request):
    if not isinstance(request, dict):
        raise ValueError('A preset must be a JSON object.')
    rows = request.get('filters')
    if not isinstance(rows, list) or not 1 <= len(rows) <= MAX_BANDS:
        raise ValueError('Use between 1 and 10 filters.')
    filters = []
    for i, row in enumerate(rows):
        if not isinstance(row, dict):
            raise ValueError(f'Filter {i + 1} must be an object.')
        kind = row.get('type', 'peak')
        kind = 'peak' if kind == 'PK' else kind
        if kind not in FILTER_TYPES:
            raise ValueError(f'Unsupported filter type: {kind}.')
        enabled = row.get('enabled', True)
        if not isinstance(enabled, bool):
            raise ValueError('Enabled must be true or false.')
        is_pass = kind in ('low_pass', 'high_pass')
        is_shelf = kind in ('low_shelf', 'high_shelf')
        gain = number(row.get('gain', 0 if is_pass else None), 'Gain', -12, 12)
        q = number(row.get('q', SHELF_Q if is_shelf or is_pass else None), 'Q', 0.1, 20)
        if is_pass and gain != 0:
            raise ValueError('Pass filters do not use gain; set gain to 0.')
        if is_shelf and not math.isclose(q, SHELF_Q, abs_tol=1e-5):
            raise ValueError('Shelf filters use fixed slope S=1 (Q=0.70710678).')
        filters.append(dict(type=kind, frequency=number(row.get('frequency'), 'Frequency', 20, 20000),
                            gain=gain, q=SHELF_Q if is_shelf else q, enabled=enabled))
    # Analytical per-stage bounds include resonant pass filters; no sampled estimate.
    headroom = sum(filter_headroom_db(f) for f in filters if f['enabled'])
    requested = request.get('preamp', 'auto')
    preamp = -headroom if requested == 'auto' else number(requested, 'Preamp', -60, 0)
    if preamp < -60:
        raise ValueError('This filter set needs more than 60 dB of headroom; reduce the boosts.')
    if preamp > -headroom + 1e-9:
        raise ValueError(f'Use Auto preamp or {(-headroom):g} dB or lower for these boosts.')
    return filters, preamp


def biquad(frequency, gain, q, rate):
    a = 10 ** (gain / 40)
    omega = 2 * math.pi * frequency / rate
    alpha = math.sin(omega) / (2 * q)
    a0 = 1 + alpha / a
    return ((1 + alpha * a) / a0, -2 * math.cos(omega) / a0,
            (1 - alpha * a) / a0, -2 * math.cos(omega) / a0, (1 - alpha / a) / a0)


def response(stages, scale, frequency, rate):
    z = cmath.exp(-2j * math.pi * frequency / rate)
    h = complex(scale)
    for b0, b1, b2, a1, a2 in stages:
        h *= (b0 + b1 * z + b2 * z * z) / (1 + a1 * z + a2 * z * z)
    return 20 * math.log10(max(abs(h), 1e-15))


def decode_words(words):
    if len(words) != 103 or words[0] != MAX_BANDS:
        raise ValueError('Unexpected vendor coefficient layout; refusing this payload.')

    def fixed(offset, bits):
        value = words[offset] << 16 | words[offset + 1]
        if value >= 2**31:
            value -= 2**32
        return value / 2**bits

    stages = []
    for band in range(MAX_BANDS):
        stage = tuple(fixed(1 + 10 * band + 2 * j, 30 if j in (1, 3) else 31) for j in range(5))
        _, _, _, a1, a2 = stage
        discriminant = cmath.sqrt(a1 * a1 - 4 * a2)
        if max(abs((-a1 + discriminant) / 2), abs((-a1 - discriminant) / 2)) >= 1:
            raise ValueError('Quantized filter is unstable; choose a lower Q or higher frequency.')
        stages.append(stage)
    scale = fixed(101, 26)
    if scale <= 0 or not math.isfinite(scale):
        raise ValueError('Invalid coefficient gain scale.')
    return stages, scale


class VendorCompiler:
    """Call from one worker process: the DLL has shared global state."""
    def __init__(self):
        self.library = C.CDLL(str(verified_dll()))
        definitions = {
            'setParam': ([C.c_int, C.c_double, C.c_int, C.c_int, C.c_int, C.c_int], None),
            'set_peq_point': ([C.c_int, C.c_int, C.c_double, C.c_double, C.c_double], None),
            'set_xpf_point': ([C.c_int, C.c_int, C.c_double, C.c_double, C.c_int], None),
            'generate_cofe': ([C.c_int], None),
            'change_rescale_cofe': ([C.c_int, C.c_double], None),
            'get_cofe_count': ([C.c_int], C.c_ushort),
            'get_cofe_param': ([C.c_int], C.POINTER(C.c_ushort)),
            'destroy': ([C.c_int], None),
        }
        for name, (args, restype) in definitions.items():
            function = getattr(self.library, name)
            function.argtypes, function.restype = args, restype

    def bank(self, filters, preamp, rate):
        lib = self.library
        lib.setParam(0, float(rate), MAX_BANDS, 1, 0, 0)
        try:
            for i in range(MAX_BANDS):
                f = filters[i] if i < len(filters) and filters[i]['enabled'] else dict(frequency=1000., gain=0., q=1.)
                # Static evidence: alpha = sin(omega) / (2 * (frequency / width)).
                kind = f.get('type', 'peak')
                if kind in ('low_pass', 'high_pass'):
                    lib.set_xpf_point(0, i, f['frequency'], f['q'], 1 if kind == 'low_pass' else 2)
                elif kind == 'peak':
                    lib.set_peq_point(0, i, f['frequency'], f['gain'], f['frequency'] / f['q'])
                else:
                    raise ValueError('Shelf filters require the extended encoder.')
            lib.generate_cofe(0)
            lib.change_rescale_cofe(0, preamp)
            count = lib.get_cofe_count(0)
            if count != 103:
                raise ValueError(f'Unexpected coefficient count: {count}.')
            return list(lib.get_cofe_param(0)[:count])
        finally:
            lib.destroy(0)


def compile_preset(request):
    filters, preamp = validate(request)
    compiler = VendorCompiler()
    extended = any(f['enabled'] and f['type'] in ('low_shelf', 'high_shelf') for f in filters)
    experimental = any(f['enabled'] and f['type'] != 'peak' for f in filters)
    banks, curves = [], []
    blob = bytearray(struct.pack('<I', len(RATES)))
    # Dense log grid plus center/near-center probes detects narrow-filter errors.
    frequencies = {20 * 1000 ** (i / 767) for i in range(768)}
    for f in filters:
        for multiplier in (0.5, 0.9, 0.99, 1, 1.01, 1.1, 2):
            frequencies.add(min(20000, max(20, f['frequency'] * multiplier)))
    frequencies = sorted(frequencies)
    max_error = 0.
    for rate_id, rate in RATES:
        words = encode_bank(filters, preamp, rate) if extended else compiler.bank(filters, preamp, rate)
        stages, scale = decode_words(words)
        ideal = [rbj_biquad(f, rate) for f in filters if f['enabled']]
        points, error = [], 0.
        probes = set(frequencies)
        for f in filters:
            if f['enabled'] and f['type'] in ('low_pass', 'high_pass') and f['q'] > SHELF_Q:
                ratio = math.sqrt(1 - 1 / (2 * f['q'] ** 2))
                ratio = ratio if f['type'] == 'low_pass' else 1 / ratio
                probes.add(rate / math.pi * math.atan(math.tan(math.pi * f['frequency'] / rate) * ratio))
        # Endpoints check shelf plateaus / passband gain; floor makes deep stopband
        # nulls meaningful to compare without dividing by a nearly-zero response.
        for frequency in sorted(probes | ({0., rate / 2} if experimental else set())):
            actual = response(stages, scale, frequency, rate)
            expected = response(ideal, 10 ** (preamp / 20), frequency, rate)
            error = max(error, abs(max(actual, -90) - max(expected, -90)) if experimental else abs(actual - expected))
            if actual > 0.15:
                raise ValueError('Compiled response exceeds available headroom.')
            if 20 <= frequency <= 20000:
                points.append([frequency, actual])
        if error > 0.5:
            raise ValueError(f'Coefficient precision error is {error:.3f} dB at {rate} Hz; try a higher frequency, smaller gain, or lower Q where adjustable.')
        max_error = max(max_error, error)
        banks.append(dict(id=rate_id, rate=rate, words=words))
        curves.append(dict(rate=rate, points=points, max_error_db=error))
        blob += struct.pack('<HH', rate_id, len(words))
        blob += struct.pack('<' + 'H' * len(words), *words)
    if len(blob) != 844:
        raise ValueError('Unexpected multi-rate payload size.')
    return dict(filters=filters, preamp=preamp, banks=banks, curves=curves,
                max_error_db=max_error, blob_hex=blob.hex(), sha256=hashlib.sha256(blob).hexdigest(),
                dll_sha256=DLL_SHA256, hardware_verified=False,
                experimental_filters=experimental, engine='extended-rbj' if extended else 'vendor',
                error_floor_db=-90 if experimental else None)


def main():
    import sys
    try:
        result = compile_preset(json.load(sys.stdin))
        json.dump(result, sys.stdout, allow_nan=False)
    except Exception as exc:
        json.dump({'error': str(exc)}, sys.stdout)
        sys.exit(1)


if __name__ == '__main__':
    main()
