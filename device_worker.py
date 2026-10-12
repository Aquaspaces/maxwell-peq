"""Bounded subprocess for snapshots and explicitly requested live or saved EQ."""
import hashlib
import json
import struct
import sys
from peq import RATES, decode_words
from win_hid import MaxwellPsHid

WRITE_STARTED = False
PERSISTENT_WRITE_STARTED = False
SAVED_VERIFIED = False
LAST_STATE = None


class StateChanged(ValueError):
    pass


def validate_blob(blob):
    if len(blob) != 844 or struct.unpack_from('<I', blob)[0] != 4:
        raise ValueError('Expected the verified 844-byte, four-rate coefficient format.')
    for offset, (rate_id, _) in zip(range(4, 844, 210), RATES):
        if struct.unpack_from('<HH', blob, offset) != (rate_id, 103):
            raise ValueError('Unexpected coefficient bank header.')
        decode_words(struct.unpack_from('<103H', blob, offset + 4))


def same_state(actual, expected, *, check_coefficients=True):
    if actual['device']['path'].lower() != expected['device']['path'].lower():
        raise StateChanged('The connected control interface changed. Use the connection associated with this backup.')
    keys = ('index', 'raw_ui_hex', 'saved_coefficients_hex') if check_coefficients else ('index', 'raw_ui_hex')
    for key in keys:
        if actual.get(key) != expected.get(key):
            raise StateChanged('The active or saved preset changed. Return to the backed-up custom preset before retrying.')


def execute(request):
    global WRITE_STARTED, PERSISTENT_WRITE_STARTED, SAVED_VERIFIED, LAST_STATE
    WRITE_STARTED = PERSISTENT_WRITE_STARTED = SAVED_VERIFIED = False
    LAST_STATE = None
    operation = request.get('operation')
    if operation not in ('read', 'apply', 'save', 'restore_saved'):
        raise ValueError('Unsupported device operation.')
    blob = None
    persistent = operation in ('save', 'restore_saved')
    if operation != 'read':
        blob = bytes.fromhex(request['blob_hex'])
        validate_blob(blob)
        if not request.get('expected') or request['expected'].get('slot') not in range(4):
            raise ValueError('A custom-preset backup is required before applying.')
    with MaxwellPsHid(allow_live=operation != 'read', allow_persistent=persistent,
                      transport=request.get('transport', 'usb')) as device:
        state = device.read_state()
        # Explicit saved-backup restoration may repair a damaged stored blob.
        if state.get('saved_coefficients_hex') and operation != 'restore_saved':
            validate_blob(bytes.fromhex(state['saved_coefficients_hex']))
        if operation == 'read':
            return state
        same_state(state, request['expected'], check_coefficients=operation != 'restore_saved')
        result = {}
        if persistent:
            if not device.has_storage_space():
                raise ValueError('Headset reports insufficient profile storage. No profile was saved.')
            WRITE_STARTED = PERSISTENT_WRITE_STARTED = True
            result.update(device.save_coefficients(state['slot'], blob))
            # Save returns only after a byte-for-byte read-back of the same key.
            SAVED_VERIFIED = True
            desired = dict(state, saved_coefficients_hex=blob.hex())
            LAST_STATE = desired
            saved = device.read_state()
            same_state(saved, desired)
            LAST_STATE = saved
            result['state'] = saved
        WRITE_STARTED = True
        live = device.apply_live(blob)
        result.update(accepted=live['accepted'], ack_hex=live['ack_hex'], live_applied=True,
                      persistence_write_sent=persistent)
        result['sha256'] = hashlib.sha256(blob).hexdigest()
        return result


if __name__ == '__main__':
    try:
        json.dump(execute(json.load(sys.stdin)), sys.stdout)
    except Exception as error:
        message = str(error)
        if not WRITE_STARTED and 'No EQ was sent' not in message:
            message += ' No EQ was sent.'
        json.dump({'error': message, 'write_started': WRITE_STARTED,
                   'persistent_write_started': PERSISTENT_WRITE_STARTED,
                   'saved_verified': SAVED_VERIFIED, 'state': LAST_STATE,
                   'state_changed': isinstance(error, StateChanged)}, sys.stdout)
        sys.exit(1)
