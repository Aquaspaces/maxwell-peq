import hashlib
import math
from pathlib import Path
import struct
import unittest

import peq


def preset(f=1000, gain=-3, q=1, preamp='auto'):
    return dict(filters=[dict(frequency=f, gain=gain, q=q)], preamp=preamp)


class ValidationTests(unittest.TestCase):
    def test_invalid_values(self):
        for request in (preset(q=0), preset(f=24000), preset(gain=13), preset(f=float('nan')),
                        preset(gain=6, preamp=0), preset(q=True), dict(filters=[])):
            with self.subTest(request=request), self.assertRaises(ValueError):
                peq.validate(request)

    def test_headroom_is_the_curve_peak(self):
        # Boosts at different frequencies don't stack: +5.27 dB, not the +10 dB sum.
        request = dict(filters=[dict(frequency=1000, gain=6, q=1),
                               dict(frequency=3000, gain=4, q=2),
                               dict(frequency=500, gain=-3, q=1)])
        self.assertEqual(peq.validate(request)[1], -5.27)

    def test_independent_peak_properties(self):
        for fs in (44100, 48000, 88200, 96000):
            for q in (0.5, 1, 4):
                a = peq.biquad(1000, 6, q, fs)
                b = peq.biquad(1000, -6, q, fs)
                self.assertAlmostEqual(peq.response([a], 1, 1000, fs), 6, places=8)
                for f in (20, 200, 1000, 2000, 20000):
                    self.assertAlmostEqual(peq.response([a, b], 1, f, fs), 0, places=8)


@unittest.skipUnless(peq.dll_path().is_file(), 'Inspected vendor DLL unavailable')
class VendorTests(unittest.TestCase):
    def test_quantized_response_and_multirate_packet(self):
        cases = [preset(), preset(f=80, q=0.7, gain=-6), preset(f=8000, q=4, gain=-8),
                 preset(f=1200, q=2, gain=6), preset(f=20000, q=2, gain=-3)]
        for request in cases:
            with self.subTest(request=request):
                result = peq.compile_preset(request)
                raw = bytes.fromhex(result['blob_hex'])
                self.assertEqual(len(raw), 844)
                self.assertEqual(struct.unpack_from('<I', raw)[0], 4)
                self.assertEqual(hashlib.sha256(raw).hexdigest(), result['sha256'])
                offset = 4
                for bank in result['banks']:
                    self.assertEqual(struct.unpack_from('<HH', raw, offset), (bank['id'], 103))
                    words = struct.unpack_from('<103H', raw, offset + 4)
                    self.assertEqual(list(words), bank['words'])
                    offset += 210
                    stages, scale = peq.decode_words(words)
                    self.assertAlmostEqual(peq.response(stages, scale, request['filters'][0]['frequency'], bank['rate']),
                                           request['filters'][0]['gain'] + result['preamp'], delta=0.03)
                self.assertLess(result['max_error_db'], 0.15)

    def test_q_changes_width_but_not_center_gain(self):
        lib = peq.VendorCompiler()
        levels = []
        for q in (0.5, 1, 4):
            fs = 48000
            filters, preamp = peq.validate(preset(q=q, gain=-6))
            stages, scale = peq.decode_words(lib.bank(filters, preamp, fs))
            self.assertAlmostEqual(peq.response(stages, scale, 1000, fs), -6, delta=0.01)
            levels.append(peq.response(stages, scale, 1500, fs))
        self.assertLess(levels[0], levels[1])
        self.assertLess(levels[1], levels[2])

    def test_flat_and_disabled_filters(self):
        request = preset(gain=0)
        result = peq.compile_preset(request)
        self.assertLess(max(abs(db) for c in result['curves'] for _, db in c['points']), 0.02)
        disabled = preset(gain=12)
        disabled['filters'][0]['enabled'] = False
        self.assertEqual(peq.compile_preset(disabled)['blob_hex'], result['blob_hex'])

    def test_multiple_peaks_and_restore_after_compiler_reuse(self):
        first = peq.compile_preset(preset())
        peq.compile_preset(dict(filters=[dict(frequency=f, gain=-2, q=1.2) for f in
                                        (100, 180, 300, 500, 800, 1500, 2500, 4500, 9000, 16000)]))
        self.assertEqual(peq.compile_preset(preset())['blob_hex'], first['blob_hex'])

    def test_low_frequency_precision_guard(self):
        with self.assertRaisesRegex(ValueError, 'precision error'):
            peq.compile_preset(dict(filters=[dict(frequency=20, gain=-6, q=2)]))


if __name__ == '__main__':
    unittest.main()
