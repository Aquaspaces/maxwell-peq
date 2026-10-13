"""Record the Python reference outputs that the Rust port must reproduce.

    .venv\\Scripts\\python tools\\make_golden.py

Writes crates/maxwell-core/tests/golden/*.json. Compile cases need the
inspected vendor DLL; the files hold only its numeric output, never the DLL.
"""
import json
import math
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

import apo  # noqa: E402
import peq  # noqa: E402

OUT = ROOT / 'crates' / 'maxwell-core' / 'tests' / 'golden'
SHELF_Q = math.sqrt(0.5)


def row(kind, frequency, gain=0, q=SHELF_Q, **extra):
    return dict(type=kind, frequency=frequency, gain=gain, q=q, **extra)


def peak(frequency=1000, gain=-3, q=1, preamp='auto'):
    return dict(filters=[dict(frequency=frequency, gain=gain, q=q)], preamp=preamp)


def outcome(function, *args):
    try:
        return dict(result=function(*args))
    except ValueError as error:
        return dict(error=str(error))


COMPILE = {
    'one_peak': peak(),
    'low_wide_cut': peak(80, -6, 0.7),
    'narrow_treble_cut': peak(8000, -8, 4),
    'boost': peak(1200, 6, 2),
    'top_edge': peak(20000, -3, 2),
    'flat': peak(gain=0),
    'disabled_boost': dict(filters=[dict(frequency=1000, gain=12, q=1, enabled=False)]),
    'legacy_pk': dict(filters=[dict(type='PK', frequency=2500, gain=-4.5, q=1.4)]),
    'manual_preamp': peak(1000, 6, 1, preamp=-8.5),
    'ten_peaks': dict(filters=[dict(frequency=f, gain=-2, q=1.2) for f in
                               (100, 180, 300, 500, 800, 1500, 2500, 4500, 9000, 16000)]),
    'precision_guard': dict(filters=[dict(frequency=f, gain=-2, q=1.2) for f in
                                     (40, 80, 150, 300, 600, 1200, 2400, 4800, 9600, 16000)]),
    'low_pass': dict(filters=[row('low_pass', 4000)]),
    'high_pass': dict(filters=[row('high_pass', 100)]),
    'low_shelf': dict(filters=[row('low_shelf', 100, 6)]),
    'high_shelf': dict(filters=[row('high_shelf', 4000, -6)]),
    'resonant_low_pass': dict(filters=[row('low_pass', 1000, q=4)]),
    'resonant_high_pass': dict(filters=[row('high_pass', 1000, q=4)]),
    'mixed': dict(filters=[row('low_shelf', 120, 6), row('peak', 1500, -5, 3), row('high_shelf', 6000, -4),
                           row('low_pass', 16000), row('high_pass', 45)]),
    'disabled_shelf': dict(filters=[row('low_shelf', 30, 12, enabled=False)]),
    'shelf_precision_guard': dict(filters=[row('low_shelf', 30, -6)]),
    'ten_with_shelves': dict(filters=[row('low_shelf', 105, 5.5), row('peak', 180, -2.4, 1.1), row('peak', 450, 1.5, 0.9),
                                      row('peak', 900, -1, 2), row('peak', 1800, 2.2, 1.6), row('peak', 3200, -3.1, 4),
                                      row('peak', 5200, 1.2, 3), row('peak', 7400, -2, 5), row('high_shelf', 9000, -2),
                                      row('low_pass', 18000)]),
    'shelf_manual_preamp': dict(filters=[row('high_shelf', 3000, 4)], preamp=-5),
}
for path in sorted((ROOT / 'examples').glob('*.json')):
    COMPILE[f'example_{path.stem}'] = json.loads(path.read_text(encoding='utf-8'))

INVALID = [
    peak(q=0), peak(frequency=24000), peak(gain=13), peak(gain=6, preamp=0), peak(q=True), dict(filters=[]),
    dict(filters=[dict(frequency=1000, gain=0, q=1)] * 11), dict(filters=['x']), dict(filters=[row('notch', 1000)]),
    dict(filters=[dict(frequency=1000, gain=0, q=1, enabled='yes')]), dict(filters=[row('low_pass', 1000, 3)]),
    dict(filters=[row('high_shelf', 1000, 3, 4)]), peak(preamp='loud'), peak(preamp=-61), peak(gain=6.02, preamp=-6),
    dict(filters=[dict(frequency=1000 + i, gain=12, q=1) for i in range(6)]), [1, 2],
    dict(filters=[dict(gain=0, q=1)]), dict(filters=[dict(frequency=1000, q=1)]), dict(filters=[dict(frequency=1000, gain=1, q=None)]),
    dict(filters=[dict(frequency=1000, gain=0.5, q=1)], preamp=-0.4), peak(gain=-3, preamp=0.5), 'text',
    dict(filters=[row('high_pass', 1000, q=0.05)]), dict(filters=[dict(frequency=19.9, gain=0, q=1)]),
]

APO_SAMPLE = """# AutoEQ style preset
Preamp: -6.2 dB
Filter 1: ON LSC Fc 105 Hz Gain 6.0 dB Q 0.70
Filter 2: ON PK Fc 180 Hz Gain -2.4 dB Q 1.10
Filter 3: OFF PK Fc 6000 Hz Gain 1 dB Q 4
Filter 4: ON BP Fc 500 Hz Q 2
Filter 5: ON HPQ Fc 30 Hz Q 0.7
Device: Speakers
"""
APO_PARSE = [
    APO_SAMPLE,
    'Filter: ON PK Fc 25000 Hz Gain 20 dB Q 50',
    'Filter 1: ON PK Fc 1000 Hz Gain -3 dB BW Oct 1',
    '\n'.join(f'Filter {i}: ON PK Fc {100 + i * 50} Hz Gain 1 dB Q 1' for i in range(1, 13)),
    '', 'Preamp: -3 dB', 'GraphicEQ: 20 0; 40 1',
    'Preamp: -2 dB\r\nPreamp: -1,5 dB\r\nFilter 1: ON PK Fc 1000,5 Hz Gain -3,25 dB Q 1,414\r\n',
    'Filter 1: ON LS 6 dB Fc 100 Hz Gain 3 dB\nFilter 2: ON LSC 12 dB Fc 100 Hz Gain 3 dB Q 0.71\n'
    'Filter 3: ON HSC 9 dB Fc 8000 Hz Gain -2 dB\nFilter 4: ON HS Fc 8000 Hz Gain -2 dB Q 2',
    'Filter 1: ON LP Fc 18000 Hz Gain 3 dB\nFilter 2: ON HP Fc 10 Hz Q 30\nFilter 3: ON PK Fc 300 Hz\n'
    'Filter 4: ON PK Fc 300 Hz Gain 2 dB\nFilter 5: ON NO Fc 60 Hz\nFilter 6: ON XYZ Fc 60 Hz\n'
    'Filter 7: ON PK Gain 3 dB Q 1\nFilter 8: on modal Fc 75 Hz Gain -4 dB Q 9\nFilter 9: ON PEQ Fc 0.5 Hz Gain -14 dB Q 0.01',
    'Copy: L=R\nInclude: other.txt\n  # only a comment\nChannel: L\nFilter 1: ON PK Fc 2000 Hz Gain 1.234567 dB Q 0.123456',
    'Filter 1: ON HSC Fc 4000 Hz Gain -30 dB Q 0.7071\nFilter 2: ON LPQ Fc 15000 Hz Q 0.5\nFilter 3: ON LS1 Fc 100 Hz Gain 2 dB',
    '{"filters": [{"type": "PK", "frequency": 1000, "gain": -3, "q": 1}, {"type": "low_pass", "frequency": 9000, "gain": 4},'
    ' {"type": "high_shelf", "frequency": 5000, "gain": 2, "q": 3, "enabled": false}], "preamp": -4}',
    '{"filters": [' + ', '.join('{"frequency": %d, "gain": 1, "q": 1}' for _ in range(12)) % tuple(range(100, 1300, 100)) + ']}',
    '{"filters": []}', '{"presets": 1}', '{"filters": [{"type": "notch", "frequency": 1}]}', '{"filters": [3]}',
    'Preamp: 2 dB\nFilter 1: OFF PK Fc 1000 Hz Gain 0 dB Q 1',
]
APO_EXPORT = [
    (dict(preamp='auto', filters=[row('low_shelf', 105, 5.5, enabled=True), row('peak', 1450, 2.1, 2, enabled=False),
                                  row('low_pass', 16000, 0, 0.7071, enabled=True)]), -7.6),
    (dict(preamp=-3.25, filters=[row('peak', 1000.123456, -0.00001, 1.4142135)]), None),
    (dict(preamp='auto', filters=[row('high_shelf', 8000, -2), row('high_pass', 20, 0, 0.5)]), None),
    (dict(preamp=0, filters=[row('peak', 20000, 12, 20, enabled=True)]), -12),
]

G_VALUES = [0, -0.0, 1, -1, 0.1, 0.5, 1.5, 12, -12, 20, 20000, -60, 0.70710678, math.sqrt(0.5), -6.2, -6.020599913279624,
            -10, 123456, 1234567, 999999.5, 9999995, 0.0001, 0.00001234, 1e-5, 1.23456789e-7, 1e16, 2.5e300, -1.5e-300,
            100.000001, 0.30000000000000004, 72, -72.00000000001, 3.14159265, 25000, 50, 0.05, 1000.5, -0.004, 314159.26]


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    compile_cases = [dict(name=name, request=request, **outcome(peq.compile_preset, request))
                     for name, request in COMPILE.items()]
    for case in compile_cases:
        # Full curves are ~3 MB; every 16th point plus the count still pins the probe grid.
        for curve in case.get('result', {}).get('curves', []):
            curve['point_count'] = len(curve['points'])
            curve['points'] = curve['points'][::16]
    validate_cases = [dict(request=request, **outcome(peq.validate, request)) for request in INVALID]
    for case in validate_cases:
        if 'result' in case:
            filters, preamp = case.pop('result')
            case['result'] = dict(filters=filters, preamp=preamp)
    files = {
        'compile.json': dict(dll_sha256=peq.DLL_SHA256, cases=compile_cases),
        'validate.json': validate_cases,
        'apo.json': dict(
            parse=[dict(text=text, **outcome(apo.parse, text)) for text in APO_PARSE],
            export=[dict(preset=preset, preamp_db=preamp_db, text=apo.export(preset, preamp_db))
                    for preset, preamp_db in APO_EXPORT]),
        'format_g.json': [[value, format(value, 'g')] for value in G_VALUES],
    }
    for name, value in files.items():
        indent = None if name == 'compile.json' else 1  # compile.json is mostly coefficient words
        (OUT / name).write_text(json.dumps(value, indent=indent, allow_nan=False) + '\n', encoding='utf-8')
        print(f'wrote {OUT / name}')


if __name__ == '__main__':
    main()
