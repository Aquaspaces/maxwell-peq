"""RBJ biquads and the Maxwell's 24-bit coefficient serialization.

Shelves use the monotonic S=1 response from the W3C Audio EQ Cookbook:
https://www.w3.org/TR/audio-eq-cookbook/

This encoder intentionally does not reproduce the vendor's stage sorting and
normalization. It uses the same numeric layout with conservative stage gain
bounds; callers must check decoded stability and response before live use.
"""
from __future__ import annotations

import math

SHELF_Q = math.sqrt(0.5)
MAX_BANDS = 10
_POSITIVE_24_MAX = (1 << 23) - 1


def filter_headroom_db(f):
    """Upper bound on a single filter's gain over DC through Nyquist."""
    if not f.get('enabled', True):
        return 0.0
    kind = f.get('type', 'peak')
    if kind in ('peak', 'low_shelf', 'high_shelf'):
        return max(0.0, f['gain'])
    if kind in ('low_pass', 'high_pass'):
        q = f['q']
        # BLT maps the analog prototype's entire frequency axis to DC..Nyquist.
        # A resonant second-order pass filter peaks at Q/sqrt(1-1/(4Q^2)).
        return (20 * math.log10(q / math.sqrt(1 - 1 / (4 * q * q)))
                if q > SHELF_Q else 0.0)
    raise ValueError(f'Unsupported filter type: {kind}')


def rbj_biquad(f, rate):
    """Return a0-normalized b0, b1, b2, a1, a2 for one validated filter."""
    if not f.get('enabled', True):
        return (1.0, 0.0, 0.0, 0.0, 0.0)
    kind = f.get('type', 'peak')
    omega = 2 * math.pi * f['frequency'] / rate
    cosine, sine = math.cos(omega), math.sin(omega)
    q = SHELF_Q if kind in ('low_shelf', 'high_shelf') else f['q']
    alpha = sine / (2 * q)
    if kind == 'peak':
        a = 10 ** (f['gain'] / 40)
        b0, b1, b2 = 1 + alpha * a, -2 * cosine, 1 - alpha * a
        a0, a1, a2 = 1 + alpha / a, -2 * cosine, 1 - alpha / a
    elif kind == 'low_pass':
        b0 = b2 = (1 - cosine) / 2
        b1 = 1 - cosine
        a0, a1, a2 = 1 + alpha, -2 * cosine, 1 - alpha
    elif kind == 'high_pass':
        b0 = b2 = (1 + cosine) / 2
        b1 = -(1 + cosine)
        a0, a1, a2 = 1 + alpha, -2 * cosine, 1 - alpha
    elif kind in ('low_shelf', 'high_shelf'):
        a = 10 ** (f['gain'] / 40)
        beta = 2 * math.sqrt(a) * alpha
        if kind == 'low_shelf':
            b0 = a * ((a + 1) - (a - 1) * cosine + beta)
            b1 = 2 * a * ((a - 1) - (a + 1) * cosine)
            b2 = a * ((a + 1) - (a - 1) * cosine - beta)
            a0 = (a + 1) + (a - 1) * cosine + beta
            a1 = -2 * ((a - 1) + (a + 1) * cosine)
            a2 = (a + 1) + (a - 1) * cosine - beta
        else:
            b0 = a * ((a + 1) + (a - 1) * cosine + beta)
            b1 = -2 * a * ((a - 1) + (a + 1) * cosine)
            b2 = a * ((a + 1) + (a - 1) * cosine - beta)
            a0 = (a + 1) - (a - 1) * cosine + beta
            a1 = 2 * ((a - 1) - (a + 1) * cosine)
            a2 = (a + 1) - (a - 1) * cosine - beta
    else:
        raise ValueError(f'Unsupported filter type: {kind}')
    return tuple(v / a0 for v in (b0, b1, b2, a1, a2))


def _packed_fixed(value, bits):
    """Round on the signed 24-bit grid and place it in the high 24 of 32 bits.

    Vendor code rounds with floor(x + .5), not bankers' rounding. Coefficients
    use fractional bits 23/22; the global gain uses 18. The wire representation
    is the resulting integer shifted left 8, high 16-bit word then low word.
    """
    if not math.isfinite(value):
        raise ValueError('Nonfinite filter coefficient.')
    scaled = math.floor(value * (1 << bits) + 0.5)
    if not -(1 << 23) <= scaled <= _POSITIVE_24_MAX:
        raise ValueError('Filter coefficient exceeds the signed 24-bit range.')
    packed = (scaled << 8) & 0xffffffff
    return [packed >> 16, packed & 0xffff]


def encode_bank(filters, preamp, rate):
    """Encode ten stages and a global gain as 103 uint16 words.

    Normalize each numerator so its ideal frequency gain cannot exceed unity,
    then compensate the accumulated scaling in the final gain. The first
    stage also gets the vendor's 1.19 attenuation factor. Positive unity is not
    exactly representable in Q23, so reserve one LSB before rounding.
    """
    active = [f for f in filters if f.get('enabled', True)]
    if len(active) > MAX_BANDS:
        raise ValueError('Use no more than 10 filters.')
    words = [MAX_BANDS]
    scale = 10 ** (preamp / 20)
    positive_limit = _POSITIVE_24_MAX / (1 << 23)
    for index in range(MAX_BANDS):
        if index < len(active):
            f = active[index]
            b0, b1, b2, a1, a2 = rbj_biquad(f, rate)
            bound = 10 ** (filter_headroom_db(f) / 20)
        else:
            b0, b1, b2, a1, a2 = (1., 0., 0., 0., 0.)
            bound = 1.
        normalization = max(bound, abs(b0), abs(b1) / 2, abs(b2)) / positive_limit
        if index == 0:
            normalization *= 1.19
        stage = (b0 / normalization, b1 / normalization, b2 / normalization, a1, a2)
        for value, bits in zip(stage, (23, 22, 23, 22, 23)):
            words.extend(_packed_fixed(value, bits))
        scale *= normalization
    # Q18 global gain has a practical positive range below 32, encoded as Q26.
    if scale <= 0:
        raise ValueError('Invalid global filter gain.')
    words.extend(_packed_fixed(scale, 18))
    if words[-2:] == [0, 0]:
        raise ValueError('Preamp is below the global gain resolution.')
    return words
