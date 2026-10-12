import ctypes as C
import math
import unittest

from extended_filters import (SHELF_Q, _packed_fixed, encode_bank,
                              filter_headroom_db, rbj_biquad)
from peq import RATES, VendorCompiler, biquad, decode_words, dll_path, response


def row(kind, frequency=1000., gain=0., q=SHELF_Q):
    return dict(type=kind, frequency=frequency, gain=gain, q=q, enabled=True)


class ExtendedFilterTests(unittest.TestCase):
    def test_shelf_endpoints_midpoint_and_monotonicity(self):
        for kind in ('low_shelf', 'high_shelf'):
            for gain in (-12., -6., 0., 6., 12.):
                for _, rate in RATES:
                    stage = rbj_biquad(row(kind, gain=gain), rate)
                    dc = response([stage], 1., 0., rate)
                    nyquist = response([stage], 1., rate / 2, rate)
                    self.assertAlmostEqual(dc, gain if kind == 'low_shelf' else 0., places=8)
                    self.assertAlmostEqual(nyquist, gain if kind == 'high_shelf' else 0., places=8)
                    self.assertAlmostEqual(response([stage], 1., 1000., rate), gain / 2, places=8)
                    ys = [response([stage], 1., rate * i / 400, rate) for i in range(201)]
                    self.assertTrue(all(min(0., gain) - 1e-8 <= y <= max(0., gain) + 1e-8 for y in ys))

    def test_pass_cutoff_resonance_and_peak_compatibility(self):
        for _, rate in RATES:
            for kind in ('low_pass', 'high_pass'):
                for q in (0.1, SHELF_Q, 1., 4., 20.):
                    f = row(kind, q=q)
                    stage = rbj_biquad(f, rate)
                    self.assertAlmostEqual(response([stage], 1., 1000., rate), 20 * math.log10(q), places=8)
                    bound = filter_headroom_db(f)
                    if q > SHELF_Q:
                        u = math.sqrt(1 - 1 / (2 * q * q))
                        if kind == 'high_pass':
                            u = 1 / u
                        peak_frequency = rate / math.pi * math.atan(u * math.tan(math.pi * 1000 / rate))
                        self.assertAlmostEqual(response([stage], 1., peak_frequency, rate), bound, places=8)
            f = row('peak', gain=6., q=4.)
            for a, b in zip(rbj_biquad(f, rate), biquad(1000., 6., 4., rate)):
                self.assertAlmostEqual(a, b, places=14)

    def test_numeric_grid_matches_vendor_representation(self):
        self.assertEqual(_packed_fixed(0.5, 23), [0x4000, 0])
        self.assertEqual(_packed_fixed(-1., 23), [0x8000, 0])
        self.assertEqual(_packed_fixed(1., 18), [0x0400, 0])
        # Half-way cases round toward positive infinity, as floor(x+.5).
        self.assertEqual(_packed_fixed(-0.5 / 2**23, 23), [0, 0])
        with self.assertRaises(ValueError):
            _packed_fixed(1., 23)
        with self.assertRaises(ValueError):
            _packed_fixed(32., 18)

    def test_mixed_cascade_quantization_and_stage_headroom(self):
        filters = [row('low_shelf', 120., 6.), row('peak', 1500., -5., 3.),
                   row('high_shelf', 6000., -4.), row('low_pass', 16000.),
                   row('high_pass', 45.)]
        preamp = -sum(filter_headroom_db(f) for f in filters)
        for _, rate in RATES:
            words = encode_bank(filters, preamp, rate)
            stages, scale = decode_words(words)
            self.assertEqual(len(words), 103)
            self.assertTrue(all(words[i] & 255 == 0 for i in range(2, 103, 2)))
            ideal = [rbj_biquad(f, rate) for f in filters]
            for i in range(768):
                frequency = 20 * 1000 ** (i / 767)
                expected = response(ideal, 10 ** (preamp / 20), frequency, rate)
                actual = response(stages, scale, frequency, rate)
                self.assertLess(abs(actual - expected), .15)
                for stage in stages:
                    self.assertLessEqual(response([stage], 1., frequency, rate), .05)

    @unittest.skipUnless(dll_path().is_file(), 'Inspected vendor DLL unavailable')
    def test_vendor_pass_api_matches_reference(self):
        compiler = VendorCompiler()
        lib = compiler.library
        lib.set_xpf_point.argtypes = [C.c_int, C.c_int, C.c_double, C.c_double, C.c_int]
        lib.set_xpf_point.restype = None
        for kind, type_id in (('low_pass', 1), ('high_pass', 2)):
            for _, rate in RATES:
                lib.setParam(0, float(rate), 10, 1, 0, 0)
                try:
                    for i in range(10):
                        lib.set_peq_point(0, i, 1000., 0., 1000.)
                    lib.set_xpf_point(0, 0, 1000., SHELF_Q, type_id)
                    lib.generate_cofe(0)
                    lib.change_rescale_cofe(0, 0.)
                    words = list(lib.get_cofe_param(0)[:lib.get_cofe_count(0)])
                    stages, scale = decode_words(words)
                    reference = [rbj_biquad(row(kind), rate)]
                    for i in range(128):
                        frequency = 20 * 1000 ** (i / 127)
                        self.assertLess(abs(response(stages, scale, frequency, rate) - response(reference, 1., frequency, rate)), .003)
                finally:
                    lib.destroy(0)


if __name__ == '__main__':
    unittest.main()
