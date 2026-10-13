"""Equalizer APO text <-> Maxwell preset conversion. No device I/O.

Import is lenient: anything the headset cannot reproduce is skipped or
clamped, and each change is reported as a warning so the user can see what
differs from the source file.
"""
from __future__ import annotations

import json
import math
import re

SHELF_Q = math.sqrt(0.5)
MAX_BANDS = 10
TYPES = {
    'PK': 'peak', 'PEQ': 'peak', 'MODAL': 'peak',
    'LP': 'low_pass', 'LPQ': 'low_pass', 'HP': 'high_pass', 'HPQ': 'high_pass',
    'LS': 'low_shelf', 'LSC': 'low_shelf', 'HS': 'high_shelf', 'HSC': 'high_shelf',
}
UNSUPPORTED = {'BP': 'band-pass', 'NO': 'notch', 'AP': 'all-pass', 'LP1': 'first-order low-pass',
               'HP1': 'first-order high-pass', 'LS1': 'first-order low shelf', 'HS1': 'first-order high shelf'}
EXPORT = {'peak': 'PK', 'low_pass': 'LPQ', 'high_pass': 'HPQ', 'low_shelf': 'LSC', 'high_shelf': 'HSC'}
FILTER_LINE = re.compile(r'^filter\s*\d*\s*:\s*(?P<state>on|off)\s+(?P<type>[a-z]+\d?)(?P<rest>.*)$', re.I)
NUMBER = r'([-+]?\d+(?:[.,]\d+)?)'


def _value(rest, label):
    match = re.search(label + r'\s+' + NUMBER, rest, re.I)
    return float(match.group(1).replace(',', '.')) if match else None


def _clamp(value, low, high, what, line, warnings):
    if value < low or value > high:
        clamped = min(high, max(low, value))
        warnings.append(f'Line {line}: {what} {value:g} is outside {low:g} to {high:g}; using {clamped:g}.')
        return clamped
    return value


def _round(value, places):
    return float(f'{value:.{places}f}')


def _from_json(text):
    data = json.loads(text)
    if not isinstance(data, dict) or not isinstance(data.get('filters'), list):
        raise ValueError('JSON presets need a "filters" list.')
    filters = []
    for i, row in enumerate(data['filters'][:MAX_BANDS]):
        if not isinstance(row, dict):
            raise ValueError(f'Filter {i + 1} must be an object.')
        kind = row.get('type', 'peak')
        kind = 'peak' if kind == 'PK' else kind
        if kind not in EXPORT:
            raise ValueError(f'Filter {i + 1}: unknown filter type.')
        is_pass, is_shelf = kind in ('low_pass', 'high_pass'), kind in ('low_shelf', 'high_shelf')
        filters.append(dict(type=kind, frequency=row.get('frequency'), gain=0 if is_pass else row.get('gain'),
                            q=SHELF_Q if is_shelf else row.get('q', SHELF_Q if is_pass else None),
                            enabled=row.get('enabled', True) is not False))
    if not filters:
        raise ValueError('The preset has no filters.')
    warnings = []
    if len(data['filters']) > MAX_BANDS:
        warnings.append(f'Only the first {MAX_BANDS} of {len(data["filters"])} filters were imported.')
    return dict(filters=filters, preamp=data.get('preamp', 'auto'), source_preamp=None, warnings=warnings)


def parse(text):
    """Return filters, a suggested preamp, the file's preamp, and warnings."""
    if not isinstance(text, str):
        raise ValueError('Paste Equalizer APO text or choose a .txt file.')
    if len(text) > 200_000:
        raise ValueError('That file is too large for an Equalizer APO preset.')
    if text.lstrip().startswith('{'):
        return _from_json(text)
    filters, warnings = [], []
    source_preamp = None
    for number, raw in enumerate(text.splitlines(), 1):
        line = raw.split('#', 1)[0].strip()
        if not line:
            continue
        lower = line.lower()
        if lower.startswith('preamp'):
            value = _value(line, r'preamp\s*:')
            if value is not None:
                source_preamp = (source_preamp or 0) + value
            continue
        if lower.startswith('graphiceq'):
            raise ValueError('GraphicEQ presets are not parametric. Export a parametric (PK filter) preset instead.')
        match = FILTER_LINE.match(line)
        if not match:
            command = line.split(':', 1)[0].strip() or line
            warnings.append(f'Line {number}: ignored "{command[:40]}"; only Preamp and Filter lines are used.')
            continue
        code = match['type'].upper()
        rest = match['rest']
        if code in UNSUPPORTED:
            warnings.append(f'Line {number}: skipped a {UNSUPPORTED[code]} filter; the headset cannot use it.')
            continue
        if code not in TYPES:
            warnings.append(f'Line {number}: skipped unknown filter type {code}.')
            continue
        kind = TYPES[code]
        slope = re.match(r'^\s*' + NUMBER + r'\s*dB', rest, re.I)
        if slope and code in ('LS', 'HS'):
            warnings.append(f'Line {number}: skipped a {code} {slope.group(1)} dB corner-frequency shelf; only center-frequency S=1 shelves are supported.')
            continue
        if slope:
            rest = rest[slope.end():]
            if float(slope.group(1).replace(',', '.')) != 12:
                warnings.append(f'Line {number}: shelf slope {slope.group(1)} dB changed to the headset\'s fixed S=1 slope.')
        frequency = _value(rest, r'fc')
        if frequency is None:
            warnings.append(f'Line {number}: skipped a filter with no Fc frequency.')
            continue
        frequency = _clamp(frequency, 20, 20000, 'Frequency', number, warnings)
        gain = _value(rest, r'gain')
        q = _value(rest, r'\bq')
        bandwidth = _value(rest, r'bw\s+oct')
        if q is None and bandwidth is not None and bandwidth > 0:
            q = math.sqrt(2 ** bandwidth) / (2 ** bandwidth - 1)
        if kind in ('low_pass', 'high_pass'):
            if gain:
                warnings.append(f'Line {number}: pass filters have no gain; ignored {gain:g} dB.')
            gain, q = 0.0, SHELF_Q if q is None else _clamp(q, 0.1, 20, 'Q', number, warnings)
        elif kind in ('low_shelf', 'high_shelf'):
            gain = _clamp(gain or 0.0, -12, 12, 'Gain', number, warnings)
            if q is not None and abs(q - SHELF_Q) > 0.02:
                warnings.append(f'Line {number}: shelf Q {q:g} changed to 0.707; the headset uses a fixed S=1 slope.')
            q = SHELF_Q
        else:
            if gain is None:
                warnings.append(f'Line {number}: peak had no gain; using 0 dB.')
            gain = _clamp(gain or 0.0, -12, 12, 'Gain', number, warnings)
            if q is None:
                warnings.append(f'Line {number}: peak had no Q; using 1.')
                q = 1.0
            q = _clamp(q, 0.1, 20, 'Q', number, warnings)
        filters.append(dict(type=kind, frequency=_round(frequency, 1), gain=_round(gain, 2),
                            q=q if q == SHELF_Q else _round(q, 3),
                            enabled=match['state'].lower() == 'on'))
    if not filters:
        raise ValueError('No usable filters found. Expected lines like "Filter 1: ON PK Fc 1000 Hz Gain -3 dB Q 1".')
    if len(filters) > MAX_BANDS:
        warnings.append(f'The headset holds {MAX_BANDS} filters; imported the first {MAX_BANDS} of {len(filters)}.')
        filters = filters[:MAX_BANDS]
    if source_preamp is not None:
        warnings.append(f'The file sets Preamp {source_preamp:g} dB. Auto preamp is used instead so boosts cannot clip.')
    return dict(filters=filters, preamp='auto', source_preamp=source_preamp, warnings=warnings)


def _number(value):
    text = f'{value:.4f}'.rstrip('0').rstrip('.')
    return '0' if text == '-0' else text


def export(preset, preamp_db=None):
    """Equalizer APO text for a preset; preamp_db overrides an 'auto' preamp."""
    filters = preset.get('filters') or []
    preamp = preset.get('preamp', 'auto')
    lines = []
    if preamp != 'auto':
        lines.append(f'Preamp: {_number(float(preamp))} dB')
    elif preamp_db is not None:
        lines.append(f'Preamp: {_number(float(preamp_db))} dB')
    for i, f in enumerate(filters, 1):
        kind = f.get('type', 'peak')
        state = 'ON' if f.get('enabled', True) else 'OFF'
        line = f'Filter {i}: {state} {EXPORT[kind]} Fc {_number(f["frequency"])} Hz'
        if kind not in ('low_pass', 'high_pass'):
            line += f' Gain {_number(f["gain"])} dB'
        line += f' Q {_number(SHELF_Q if kind in ("low_shelf", "high_shelf") else f["q"])}'
        lines.append(line)
    return '\n'.join(lines) + '\n'
