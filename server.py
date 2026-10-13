"""Loopback editor. Device controls are on unless --offline; writes need explicit actions."""
from __future__ import annotations
import argparse
import copy
from datetime import datetime, timezone
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import threading
import time
import uuid

import apo
import peq

ROOT = Path(__file__).resolve().parent
AUDEZE_PROCESSES = ('Audeze.exe', 'Audeze HQ.exe', 'AudezeHQ.exe')
NO_WINDOW = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0


def data_root():
    """Profiles and backups: MAXWELL_PEQ_DATA, else the checkout."""
    override = os.environ.get('MAXWELL_PEQ_DATA')
    return Path(override) if override else ROOT


def running_audeze():
    """Names of running Audeze apps, or None where the check is unsupported."""
    if os.name != 'nt':
        return None
    try:
        output = subprocess.run(['tasklist', '/FO', 'CSV', '/NH'], capture_output=True, text=True,
                                timeout=5, creationflags=NO_WINDOW).stdout
    except (OSError, subprocess.TimeoutExpired):
        return None
    names = {line.split('","', 1)[0].strip('"').lower() for line in output.splitlines()}
    return [name for name in AUDEZE_PROCESSES if name.lower() in names]


def close_audeze():
    """Ask Audeze apps to exit, then force any that remain (tray processes ignore WM_CLOSE)."""
    running = running_audeze()
    if not running:
        return running
    for force in (False, True):
        for name in running:
            subprocess.run(['taskkill', '/IM', name, '/T'] + (['/F'] if force else []), capture_output=True,
                           timeout=10, creationflags=NO_WINDOW)
        deadline = time.monotonic() + 3
        while running and time.monotonic() < deadline:
            time.sleep(0.25)
            running = running_audeze()
        if not running:
            break
    return running


class AudezeWatch:
    """tasklist takes ~50 ms; status polling shares one recent answer."""
    def __init__(self, check=running_audeze, ttl=1.5):
        self.check, self.ttl = check, ttl
        self.lock = threading.Lock()
        self.value, self.when = None, -ttl

    def running(self, fresh=False):
        with self.lock:
            if fresh or time.monotonic() - self.when > self.ttl:
                self.value, self.when = self.check(), time.monotonic()
            return self.value

    def reset(self):
        with self.lock:
            self.when = -self.ttl


class WorkerError(ValueError):
    def __init__(self, result):
        super().__init__(result.get('error', 'Worker failed.'))
        self.result = result


def worker_command(script):
    return [sys.executable, str(ROOT / script)]


def worker(script, request, timeout=15):
    try:
        process = subprocess.run(worker_command(script), input=json.dumps(request),
                                 encoding='utf-8', capture_output=True, timeout=timeout, cwd=ROOT,
                                 creationflags=NO_WINDOW)
    except subprocess.TimeoutExpired as error:
        raise RuntimeError('Worker timed out; device acceptance is unconfirmed. Use Restore saved preset after a Save, or Restore live EQ after a temporary Apply.') from error
    try:
        result = json.loads(process.stdout)
    except (ValueError, TypeError) as error:
        raise RuntimeError('Worker did not return a valid result. Device acceptance is unconfirmed.') from error
    if process.returncode or 'error' in result:
        raise WorkerError(result)
    return result


class Profiles:
    """Named presets in one JSON file. Contents are editor drafts, so only the shape is checked."""
    FIELDS = ('type', 'frequency', 'gain', 'q', 'enabled')

    def __init__(self, path):
        self.path = Path(path) if path is not None else None
        self.lock = threading.Lock()
        self.data = dict(version=1, active=None, profiles=[])
        if self.path and self.path.exists():
            data = json.loads(self.path.read_text(encoding='utf-8'))
            if not isinstance(data, dict) or not isinstance(data.get('profiles'), list):
                raise ValueError(f'Invalid profile library: {self.path}')
            self.data = dict(version=1, active=data.get('active'),
                             profiles=[p for p in data['profiles'] if self._valid(p)])
        if not self.data['profiles']:
            flat = dict(filters=[dict(type='peak', frequency=1000, gain=0, q=1, enabled=True)], preamp='auto')
            self.data['profiles'].append(dict(id=uuid.uuid4().hex, name='Flat', preset=flat))
        if self.find(self.data['active']) is None:
            self.data['active'] = self.data['profiles'][0]['id']

    @classmethod
    def _valid(cls, profile):
        try:
            cls.check_preset(profile['preset'])
            cls.check_name(profile['name'])
            return isinstance(profile['id'], str)
        except (KeyError, TypeError, ValueError):
            return False

    @staticmethod
    def check_name(name):
        if not isinstance(name, str) or not name.strip() or len(name.strip()) > 60:
            raise ValueError('Profile names need 1 to 60 characters.')
        return name.strip()

    @classmethod
    def check_preset(cls, preset):
        if not isinstance(preset, dict) or not isinstance(preset.get('filters'), list) or not 1 <= len(preset['filters']) <= 10:
            raise ValueError('A profile needs 1 to 10 filters.')
        filters = []
        for row in preset['filters']:
            if not isinstance(row, dict) or row.get('type') not in peq.FILTER_TYPES:
                raise ValueError('Invalid filter in profile.')
            for key in ('frequency', 'gain', 'q'):
                if row.get(key) is not None and (isinstance(row[key], bool) or not isinstance(row[key], (int, float))):
                    raise ValueError('Filter values must be numbers.')
            filters.append({key: row.get(key) for key in cls.FIELDS} | dict(enabled=row.get('enabled') is not False))
        preamp = preset.get('preamp', 'auto')
        if preamp != 'auto' and (isinstance(preamp, bool) or not isinstance(preamp, (int, float))):
            raise ValueError('Preamp must be auto or a number.')
        return dict(filters=filters, preamp=preamp)

    def find(self, profile_id):
        return next((p for p in self.data['profiles'] if p['id'] == profile_id), None)

    def listing(self):
        with self.lock:
            return copy.deepcopy(self.data)

    def perform(self, path, request):
        with self.lock:
            profiles = self.data['profiles']
            if path == '/api/profiles/save':
                profile = self.find(request.get('id')) if request.get('id') else None
                name = self.check_name(request.get('name')) if 'name' in request or profile is None else profile['name']
                preset = self.check_preset(request.get('preset')) if 'preset' in request or profile is None else profile['preset']
                if profile is None:
                    if len(profiles) >= 50:
                        raise ValueError('The library holds 50 profiles. Delete one first.')
                    profile = dict(id=uuid.uuid4().hex)
                    profiles.append(profile)
                profile.update(name=name, preset=preset)
                if request.get('activate'):
                    self.data['active'] = profile['id']
                saved = profile['id']
            elif path == '/api/profiles/delete':
                if len(profiles) == 1:
                    raise ValueError('Keep at least one profile.')
                profile = self.find(request.get('id'))
                if profile is None:
                    raise ValueError('That profile no longer exists.')
                index = profiles.index(profile)
                profiles.remove(profile)
                if self.data['active'] == profile['id']:
                    self.data['active'] = profiles[min(index, len(profiles) - 1)]['id']
                saved = None
            elif path == '/api/profiles/activate':
                if self.find(request.get('id')) is None:
                    raise ValueError('That profile no longer exists.')
                self.data['active'] = saved = request['id']
            elif path == '/api/profiles/order':
                ids = request.get('ids')
                if not isinstance(ids, list) or sorted(ids) != sorted(p['id'] for p in profiles):
                    raise ValueError('The profile order does not match the library.')
                self.data['profiles'] = [self.find(i) for i in ids]
                saved = None
            else:
                raise ValueError('Unknown endpoint.')
            if self.path:
                App._atomic_json(self.path, self.data)
            return dict(copy.deepcopy(self.data), saved=saved)


class App:
    def __init__(self, enable_live=False, session_path=None, audeze=None):
        self.enable_live = enable_live
        self.audeze = audeze or AudezeWatch()
        self.token = secrets.token_urlsafe(32)
        self.lock = threading.Lock()
        self.backup = None
        self.expected = None
        self.transport = 'usb'
        self.device = 'Original Maxwell PlayStation'
        self.live_state = 'unchanged'
        self.storage_state = 'unchanged'
        self.backup_path = None
        self.pending = None
        self.last_profile = None
        self.saved_profile = None
        self.session_recovered = False
        self.session_path = Path(session_path) if session_path is not None else None
        if self.session_path and self.session_path.exists():
            self._recover()

    @staticmethod
    def _atomic_json(path, value):
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_name(path.name + '.' + secrets.token_hex(8) + '.tmp')
        try:
            with temporary.open('x', encoding='utf-8') as output:
                json.dump(value, output, indent=2, allow_nan=False)
                output.flush()
                os.fsync(output.fileno())
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)

    def _session(self):
        return dict(version=1, backup=self.backup, backup_path=self.backup_path,
                    expected=self.expected, transport=self.transport, device=self.device,
                    live_state=self.live_state, storage_state=self.storage_state, pending=self.pending,
                    last_profile=self.last_profile, saved_profile=self.saved_profile)

    def _persist(self):
        if self.session_path:
            self._atomic_json(self.session_path, self._session())

    def _recover(self):
        data = json.loads(self.session_path.read_text(encoding='utf-8'))
        if (not isinstance(data, dict) or data.get('version') != 1
                or data.get('transport') not in ('usb', 'dongle')
                or data.get('live_state') not in ('unchanged', 'applied', 'uncertain')
                or data.get('storage_state') not in ('unchanged', 'saved', 'uncertain')):
            raise ValueError('Invalid recovery session; preserve backups/session.json for recovery.')
        for key in ('backup', 'expected'):
            value = data.get(key)
            if value is not None:
                state = value.get('state') if key == 'backup' and isinstance(value, dict) else value
                if not isinstance(state, dict) or state.get('slot') not in range(4):
                    raise ValueError('Invalid preset snapshot in recovery session.')
                if len(bytes.fromhex(state.get('saved_coefficients_hex', ''))) != 844:
                    raise ValueError('Invalid saved coefficients in recovery session.')
        if (data['live_state'] != 'unchanged' or data['storage_state'] != 'unchanged') and not data.get('backup'):
            raise ValueError('Recovery session has no original preset backup.')
        if data.get('pending') is not None and not isinstance(data['pending'], dict):
            raise ValueError('Invalid pending recovery operation.')
        for key in self._session():
            if key != 'version':
                setattr(self, key, data.get(key))
        self.session_recovered = True

    def _audit(self, kind, value):
        # Tests can use an in-memory App; production keeps both snapshots and filter JSON.
        directory = self.session_path.parent if self.session_path else ROOT / 'backups'
        stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S.%fZ')
        target = directory / f'{stamp}-{kind}.json'
        self._atomic_json(target, dict(created_at=stamp, **value))
        return str(target)

    def _worker_request(self, **request):
        request['transport'] = self.transport
        return request

    def status(self):
        try:
            peq.verified_dll()
            dll_available, dll_error = True, None
        except Exception as error:
            dll_available, dll_error = False, str(error)
        audeze = self.audeze.running() or []
        return dict(token=self.token, dll_available=dll_available, dll_error=dll_error,
                    live_enabled=self.enable_live, device=self.device,
                    backup_exists=self.backup is not None, live_state=self.live_state,
                    storage_state=self.storage_state, transport=self.transport,
                    transports=['usb', 'dongle'], session_recovered=self.session_recovered,
                    pending_operation=self.pending.get('operation') if self.pending else None,
                    last_profile=self.last_profile, saved_profile=self.saved_profile,
                    audeze_running=bool(audeze), audeze_apps=audeze,
                    data_dir=str(self.session_path.parent.parent) if self.session_path else None,
                    backup_path=self.backup_path, hardware_audio_verified=False)

    def perform(self, path, request):
        if path not in ('/api/connect', '/api/read_device', '/api/apply', '/api/restore', '/api/save', '/api/restore_saved'):
            raise ValueError('Unknown endpoint.')
        if not self.enable_live:
            raise ValueError('Device controls are off because the app was started with --offline.')
        if self.audeze.running(fresh=True):
            raise ValueError('Audeze App/HQ is open. Custom PEQ will not work while it is open; close it first. No EQ was sent.')
        if path == '/api/connect':
            if self.backup is None:
                return self.perform('/api/read_device', request)
            transport = request.get('transport', self.transport)
            if transport not in ('usb', 'dongle'):
                raise ValueError('Choose USB-C or dongle.')
            if self.storage_state == 'uncertain':
                raise ValueError('Restore the uncertain saved profile through its current connection before switching.')
            state = worker('device_worker.py', dict(operation='read', transport=transport))
            expected = self.expected or self.backup['state']
            if any(state.get(key) != expected.get(key) for key in
                   ('index', 'slot', 'raw_ui_hex', 'saved_coefficients_hex')):
                raise ValueError('This connection has a different preset or stored EQ. Your existing connection and original backup are unchanged.')
            previous = (self.transport, self.device, self.expected)
            self.transport = transport
            self.device = f"Original Maxwell PS · {'USB-C' if transport == 'usb' else 'dongle'} · preset {state['index']}"
            self.expected = copy.deepcopy(state)
            try:
                self._persist()
            except Exception:
                self.transport, self.device, self.expected = previous
                raise
            return dict(state=state, backup_exists=True, backup_path=self.backup_path,
                        message='Connection verified. Your original backup is retained. No EQ was sent.')
        if path == '/api/read_device':
            if self.live_state != 'unchanged' or self.storage_state != 'unchanged':
                raise ValueError('Restore the existing backup before reading a new one.')
            transport = request.get('transport', self.transport)
            if transport not in ('usb', 'dongle'):
                raise ValueError('Choose USB-C or dongle.')
            state = worker('device_worker.py', dict(operation='read', transport=transport))
            self.transport = transport
            self.device = f"Original Maxwell PS · {'USB-C' if transport == 'usb' else 'dongle'} · preset {state['index']}"
            if state['slot'] is None:
                self.backup = None
                self.backup_path = None
                self.expected = None
                self.pending = None
                self._persist()
                return dict(state=state, backup_exists=False,
                            message=f"Headset read successfully. Preset {state['index']} is built-in. Choose a custom preset in the Audeze app, then read again.")
            blob = bytes.fromhex(state['saved_coefficients_hex'])
            backup = dict(state=copy.deepcopy(state), coefficient_sha256=hashlib.sha256(blob).hexdigest(),
                          note='Exact saved coefficients. Existing unsaved live DSP state cannot be read.')
            target = self._audit(f'slot{state["slot"] + 1}', backup)
            self.backup, self.backup_path = backup, target
            self.expected = copy.deepcopy(state)
            self.pending = None
            self._persist()
            return dict(state=state, backup_exists=True, backup_path=target,
                        message=f"Custom preset {state['slot'] + 1} backed up. Restore will reapply its exact saved coefficients.")
        if self.backup is None:
            raise ValueError('Read and back up the active custom preset first.')
        persistent = path in ('/api/save', '/api/restore_saved')
        restoring = path in ('/api/restore', '/api/restore_saved')
        if not restoring:
            if self.live_state == 'uncertain' or self.storage_state == 'uncertain':
                raise ValueError('Restore after the previous uncertain write before applying a new filter.')
            compiled = worker('peq.py', request)
            blob_hex = compiled['blob_hex']
        else:
            blob_hex = self.backup['state']['saved_coefficients_hex']
            compiled = None
        if path == '/api/restore' and self.storage_state == 'uncertain':
            raise ValueError('Saved preset state is uncertain. Use Restore saved preset to recover it.')
        previous = (self.live_state, self.storage_state)
        if self.expected is None:
            self.expected = copy.deepcopy(self.backup['state'])
        operation = 'restore_saved' if path == '/api/restore_saved' else 'save' if persistent else 'apply'
        previous_pending = self.pending
        self.pending = dict(operation=operation, blob_hex=blob_hex, preset=request if compiled else None)
        self.live_state = 'uncertain'
        if persistent:
            self.storage_state = 'uncertain'
        # A crash from this point forward must leave enough information to restore.
        try:
            self._persist()
            if persistent:
                self._audit(operation, dict(preset=request if compiled else None, blob_hex=blob_hex,
                                           original_backup=self.backup_path, expected=self.expected))
        except Exception:
            self.live_state, self.storage_state = previous
            self.pending = previous_pending
            self._persist()
            raise
        try:
            result = worker('device_worker.py', self._worker_request(operation=operation, blob_hex=blob_hex,
                                                                     expected=self.expected))
            if persistent and (not result.get('saved_verified') or not isinstance(result.get('state'), dict)
                               or result['state'].get('saved_coefficients_hex') != blob_hex):
                raise RuntimeError('Saved coefficient readback is unconfirmed. Use Restore saved preset.')
            if persistent and not result.get('live_applied'):
                raise WorkerError(dict(error='Saved coefficients were verified, but live application is unconfirmed.',
                                       write_started=True, persistent_write_started=True,
                                       saved_verified=True, state=result['state']))
        except WorkerError as error:
            details = error.result
            if details.get('write_started') is False and not details.get('persistent_write_started'):
                self.live_state, self.storage_state = previous
                self.pending = previous_pending
                if error.result.get('state_changed'):
                    if self.storage_state == 'unchanged':
                        self.backup = None
                        self.expected = None
                        self.backup_path = None
                        self.pending = None
                        self.live_state = 'unchanged'
            elif persistent and details.get('saved_verified') and isinstance(details.get('state'), dict):
                self.expected = copy.deepcopy(details['state'])
                self.storage_state = ('unchanged' if restoring else 'saved') if self.expected.get('saved_coefficients_hex') == blob_hex else 'uncertain'
                if not restoring:
                    self.last_profile = copy.deepcopy(request)
                self.saved_profile = copy.deepcopy(request) if self.storage_state == 'saved' else None
                error = WorkerError(dict(details, error=str(error) + ' Saved coefficients were read back; live EQ is unconfirmed. The original backup is intact.'))
            elif persistent and details.get('persistent_write_started') is False:
                self.storage_state = previous[1]
            self._persist()
            raise error
        except Exception:
            self._persist()
            raise
        if persistent:
            self.expected = copy.deepcopy(result['state'])
            self.storage_state = 'unchanged' if restoring else 'saved'
            self.saved_profile = None if restoring else copy.deepcopy(request)
        if not restoring:
            self.last_profile = copy.deepcopy(request)
        self.live_state = 'unchanged' if restoring else 'applied'
        self.pending = None
        self._persist()
        result['message'] = {
            '/api/apply': 'Headset acknowledged the temporary EQ. Saved presets are unchanged.',
            '/api/restore': 'Headset acknowledged the original backup in live EQ. This does not change saved storage.',
            '/api/save': 'Preset saved, read back exactly, and applied. Audeze HQ sliders still show their old values; editing them can replace this EQ.',
            '/api/restore_saved': 'Original saved coefficients restored, read back exactly, and applied.',
        }[path]
        result['live_state'], result['storage_state'] = self.live_state, self.storage_state
        return result


class Handler(BaseHTTPRequestHandler):
    FILES = {'/': ('index.html', 'text/html; charset=utf-8'), '/style.css': ('style.css', 'text/css; charset=utf-8'),
             '/app.js': ('app.js', 'text/javascript; charset=utf-8'), '/graph.js': ('graph.js', 'text/javascript; charset=utf-8')}

    def log_message(self, format, *args):
        # No query strings, filter data, device identifiers, or tokens in logs.
        pass

    def send(self, status, body, mime='application/json; charset=utf-8'):
        self.send_response(status)
        self.send_header('Content-Type', mime)
        self.send_header('Content-Length', str(len(body)))
        self.send_header('Cache-Control', 'no-store')
        self.send_header('X-Content-Type-Options', 'nosniff')
        self.send_header('Content-Security-Policy', "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'")
        self.end_headers()
        self.wfile.write(body)

    def json(self, status, value):
        self.send(status, json.dumps(value, allow_nan=False).encode('utf-8'))

    def valid_host(self):
        return self.headers.get('Host') == f'127.0.0.1:{self.server.server_port}'

    def do_GET(self):
        if not self.valid_host():
            return self.json(403, {'error': 'Use the 127.0.0.1 address printed by the server.'})
        if self.path == '/api/status':
            self.server.app.audeze.running()  # refresh the cached tasklist answer before taking the device lock
            with self.server.app.lock:
                return self.json(200, self.server.app.status())
        if self.path == '/api/profiles':
            return self.json(200, self.server.profiles.listing())
        if self.path not in self.FILES:
            return self.json(404, {'error': 'Not found.'})
        filename, mime = self.FILES[self.path]
        self.send(200, (ROOT / 'web' / filename).read_bytes(), mime)

    def handle_local(self, request):
        """Requests that never touch the headset; they must not wait for the device lock."""
        if self.path == '/api/compile':
            return worker('peq.py', request)
        if self.path.startswith('/api/profiles/'):
            return self.server.profiles.perform(self.path, request)
        if self.path == '/api/apo/parse':
            return apo.parse(request.get('text'))
        if self.path == '/api/apo/export':
            preset = Profiles.check_preset(request.get('preset'))
            return dict(text=apo.export(preset, request.get('preamp_db')))
        if self.path == '/api/close_audeze':
            still = close_audeze()
            self.server.app.audeze.reset()
            if still:
                raise ValueError(f'{", ".join(still)} is still running. Quit it from the tray icon.')
            return dict(audeze_running=bool(self.server.app.audeze.running(fresh=True)))
        return None

    def do_POST(self):
        origin = self.headers.get('Origin')
        expected_origin = f'http://127.0.0.1:{self.server.server_port}'
        if not self.valid_host() or origin not in (None, expected_origin) or not secrets.compare_digest(self.headers.get('X-Session-Token', ''), self.server.app.token):
            return self.json(403, {'error': 'Invalid local session. Reload the editor.'})
        try:
            size = int(self.headers.get('Content-Length', '0'))
            if not 1 <= size <= 262144 or self.headers.get_content_type() != 'application/json':
                raise ValueError('Expected a JSON request of at most 256 KB.')
            request = json.loads(self.rfile.read(size))
            if not isinstance(request, dict):
                raise ValueError('Expected a JSON object.')
            result = self.handle_local(request)
            if result is not None:
                return self.json(200, result)
            if not self.server.app.lock.acquire(blocking=False):
                return self.json(409, {'error': 'Another operation is running; wait for it to finish.'})
            try:
                result = self.server.app.perform(self.path, request)
            finally:
                self.server.app.lock.release()
            self.json(200, result)
        except Exception as error:
            self.json(400, {'error': str(error)})


def make_server(enable_live=True, port=0, data=None):
    data = Path(data) if data is not None else data_root()
    server = ThreadingHTTPServer(('127.0.0.1', port), Handler)
    server.daemon_threads = True
    server.app = App(enable_live, data / 'backups' / 'session.json')
    server.profiles = Profiles(data / 'profiles.json')
    return server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=8769)
    parser.add_argument('--offline', action='store_true', help='edit and preview only; never open the headset')
    parser.add_argument('--enable-live', action='store_true', help=argparse.SUPPRESS)  # default since the redesign
    args = parser.parse_args()
    server = make_server(not args.offline, args.port)
    print(f'Editor: http://127.0.0.1:{server.server_port} | Device controls: {not args.offline}', flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == '__main__':
    main()
