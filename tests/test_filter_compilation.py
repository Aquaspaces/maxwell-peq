import math
import unittest

import peq
from extended_filters import filter_headroom_db


def row(kind, frequency=1000, gain=0, q=math.sqrt(.5)):
    return dict(type=kind, frequency=frequency, gain=gain, q=q)


class FilterValidationTests(unittest.TestCase):
    def test_legacy_peak_and_type_specific_fields(self):
        self.assertEqual(peq.validate({'filters': [row('PK')]})[0][0]['type'], 'peak')
        for f in [row('unknown'), row('low_pass', gain=3), row('high_shelf', q=4)]:
            with self.subTest(filter=f), self.assertRaises(ValueError):
                peq.validate({'filters': [f]})
        f = dict(type='low_shelf', frequency=100, gain=-3)
        self.assertEqual(peq.validate({'filters': [f]})[0][0]['q'], math.sqrt(.5))

    def test_resonant_pass_filters_require_headroom(self):
        f = row('high_pass', q=4)
        filters, preamp = peq.validate({'filters': [f]})
        self.assertAlmostEqual(preamp, -filter_headroom_db(f))
        self.assertLess(preamp, -12)
        with self.assertRaises(ValueError):
            peq.validate({'filters': [f], 'preamp': 0})


@unittest.skipUnless(peq.dll_path().is_file(), 'Inspected vendor DLL unavailable')
class FilterCompilationTests(unittest.TestCase):
    def test_peak_payload_unchanged_from_listening_tested_build(self):
        result = peq.compile_preset({'filters': [dict(frequency=1000, gain=-3, q=1)]})
        self.assertEqual(result['sha256'], '2d02f080ad302fc731623d0e4ce430d47cb2daa88dc2b1ce9d7ef33900a5f635')
        self.assertFalse(result['experimental_filters'])

    def test_all_new_types_and_resonant_pass_compile(self):
        cases = [row('low_pass', 4000), row('high_pass', 100),
                 row('low_shelf', 100, 6), row('high_shelf', 4000, -6),
                 row('low_pass', 1000, q=4), row('high_pass', 1000, q=4)]
        for f in cases:
            with self.subTest(filter=f):
                result = peq.compile_preset({'filters': [f]})
                self.assertEqual(len(bytes.fromhex(result['blob_hex'])), 844)
                self.assertTrue(result['experimental_filters'])
                self.assertLess(result['max_error_db'], .15)
                self.assertTrue(all(db <= .15 for c in result['curves'] for _, db in c['points']))
                self.assertEqual(result['engine'], 'extended-rbj' if 'shelf' in f['type'] else 'vendor')

    def test_mixed_filters_and_disabled_shelf(self):
        result = peq.compile_preset({'filters': [row('low_shelf', 120, 6), row('peak', 1500, -5, 3),
                                               row('high_shelf', 6000, -4), row('low_pass', 16000),
                                               row('high_pass', 45)]})
        self.assertEqual(result['preamp'], -6)
        self.assertLess(result['max_error_db'], .15)
        f = row('low_shelf', 30, 12)
        f['enabled'] = False
        result = peq.compile_preset({'filters': [f]})
        self.assertEqual(result['engine'], 'vendor')
        self.assertEqual(result['preamp'], 0)
        self.assertFalse(result['experimental_filters'])

    def test_shelf_precision_limit_is_enforced(self):
        with self.assertRaisesRegex(ValueError, 'precision error'):
            peq.compile_preset({'filters': [row('low_shelf', 30, -6)]})
