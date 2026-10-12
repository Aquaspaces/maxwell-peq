"""Bounded subprocess for read-only snapshots or explicitly requested live writes."""
import hashlib
import json
import struct
import sys
from peq import RATES, decode_words
from win_hid import MaxwellPsHid

WRITE_STARTED = False


class StateChanged(ValueError):
    pass


def validate_blob(blob):
    if len(blob) != 844 or struct.unpack_from('<I', blob)[0] != 4:
        raise ValueError('Expected the verified 844-byte, four-rate coefficient format.')
    for offset, (rate_id, _) in zip(range(4, 844, 210), RATES):
        if struct.unpack_from('<HH', blob, offset) != (rate_id, 103):
            raise ValueError('Unexpected coefficient bank header.')
        decode_words(struct.unpack_from('<103H', blob, offset + 4))


def same_state(actual, expected):
    if actual['device']['path'].lower() != expected['device']['path'].lower():
        raise StateChanged('The connected headset changed; no EQ was sent. Read a fresh backup.')
    for key in ('index', 'raw_ui_hex', 'saved_coefficients_hex'):
        if actual.get(key) != expected.get(key):
            raise StateChanged('The active or saved preset changed. No EQ was sent; read a fresh backup.')


def execute(request):
    global WRITE_STARTED
    WRITE_STARTED = False
    operation = request.get('operation')
    if operation not in ('read', 'apply'):
        raise ValueError('Unsupported device operation.')
    blob = None
    if operation == 'apply':
        blob = bytes.fromhex(request['blob_hex'])
        validate_blob(blob)
        if not request.get('expected') or request['expected'].get('slot') not in range(4):
            raise ValueError('A custom-preset backup is required before applying.')
    with MaxwellPsHid(allow_live=operation == 'apply') as device:
        state = device.read_state()
        if state.get('saved_coefficients_hex'):
            validate_blob(bytes.fromhex(state['saved_coefficients_hex']))
        if operation == 'read':
            return state
        same_state(state, request['expected'])
        WRITE_STARTED = True
        result = device.apply_live(blob)
        result['sha256'] = hashlib.sha256(blob).hexdigest()
        return result


if __name__ == '__main__':
    try:
        json.dump(execute(json.load(sys.stdin)), sys.stdout)
    except Exception as error:
        json.dump({'error': str(error), 'write_started': WRITE_STARTED,
                   'state_changed': isinstance(error, StateChanged)}, sys.stdout)
        sys.exit(1)
