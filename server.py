"""Loopback editor. Device writes require --enable-live and explicit actions."""
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

import peq

ROOT = Path(__file__).resolve().parent


class WorkerError(ValueError):
    def __init__(self, result):
        super().__init__(result.get('error', 'Worker failed.'))
        self.result = result


def worker(script, request, timeout=15):
    try:
        process = subprocess.run([sys.executable, str(ROOT / script)], input=json.dumps(request),
                                 encoding='utf-8', capture_output=True, timeout=timeout, cwd=ROOT,
                                 creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
    except subprocess.TimeoutExpired as error:
        raise RuntimeError('Worker timed out; device acceptance is unconfirmed. Use Restore saved preset after a Save, or Restore live EQ after a temporary Apply.') from error
    try:
        result = json.loads(process.stdout)
    except (ValueError, TypeError) as error:
        raise RuntimeError('Worker did not return a valid result. Device acceptance is unconfirmed.') from error
    if process.returncode or 'error' in result:
        raise WorkerError(result)
    return result


class App:
    def __init__(self, enable_live=False, session_path=None):
        self.enable_live = enable_live
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
                    last_profile=self.last_profile)

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
        return dict(token=self.token, dll_available=dll_available, dll_error=dll_error,
                    live_enabled=self.enable_live, device=self.device,
                    backup_exists=self.backup is not None, live_state=self.live_state,
                    storage_state=self.storage_state, transport=self.transport,
                    transports=['usb', 'dongle'], session_recovered=self.session_recovered,
                    pending_operation=self.pending.get('operation') if self.pending else None,
                    last_profile=self.last_profile,
                    backup_path=self.backup_path, hardware_audio_verified=False)

    def perform(self, path, request):
        if path == '/api/compile':
            return worker('peq.py', request)
        if path not in ('/api/read_device', '/api/apply', '/api/restore', '/api/save', '/api/restore_saved'):
            raise ValueError('Unknown endpoint.')
        if not self.enable_live:
            raise ValueError('Device controls are disabled. Restart with --enable-live for a deliberate headset test.')
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
            with self.server.app.lock:
                return self.json(200, self.server.app.status())
        files = {'/': ('index.html', 'text/html; charset=utf-8'), '/style.css': ('style.css', 'text/css; charset=utf-8'),
                 '/app.js': ('app.js', 'text/javascript; charset=utf-8')}
        if self.path not in files:
            return self.json(404, {'error': 'Not found.'})
        filename, mime = files[self.path]
        self.send(200, (ROOT / 'web' / filename).read_bytes(), mime)

    def do_POST(self):
        origin = self.headers.get('Origin')
        expected_origin = f'http://127.0.0.1:{self.server.server_port}'
        if not self.valid_host() or origin not in (None, expected_origin) or not secrets.compare_digest(self.headers.get('X-Session-Token', ''), self.server.app.token):
            return self.json(403, {'error': 'Invalid local session. Reload the editor.'})
        try:
            size = int(self.headers.get('Content-Length', '0'))
            if not 1 <= size <= 65536 or self.headers.get_content_type() != 'application/json':
                raise ValueError('Expected a JSON request of at most 64 KB.')
            request = json.loads(self.rfile.read(size))
            if not isinstance(request, dict):
                raise ValueError('Expected a JSON object.')
            if not self.server.app.lock.acquire(blocking=False):
                return self.json(409, {'error': 'Another operation is running; wait for it to finish.'})
            try:
                result = self.server.app.perform(self.path, request)
            finally:
                self.server.app.lock.release()
            self.json(200, result)
        except Exception as error:
            self.json(400, {'error': str(error)})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=8769)
    parser.add_argument('--enable-live', action='store_true')
    args = parser.parse_args()
    server = ThreadingHTTPServer(('127.0.0.1', args.port), Handler)
    server.app = App(args.enable_live, ROOT / 'backups' / 'session.json')
    print(f'Editor: http://127.0.0.1:{server.server_port} | Device controls: {args.enable_live}', flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == '__main__':
    main()
