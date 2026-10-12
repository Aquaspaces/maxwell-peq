"""Loopback editor. Device writes require --enable-live and explicit Apply/Restore."""
from __future__ import annotations
import argparse
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
        raise RuntimeError('Worker timed out. If this was Apply or Restore, live EQ state is uncertain; use Restore or reselect the saved preset in Audeze.') from error
    try:
        result = json.loads(process.stdout)
    except (ValueError, TypeError) as error:
        raise RuntimeError('Worker did not return a valid result. Device acceptance is unconfirmed.') from error
    if process.returncode or 'error' in result:
        raise WorkerError(result)
    return result


class App:
    def __init__(self, enable_live=False):
        self.enable_live = enable_live
        self.token = secrets.token_urlsafe(32)
        self.lock = threading.Lock()
        self.backup = None
        self.device = 'Original Maxwell PlayStation · USB-C required'
        self.live_state = 'unchanged'
        self.backup_path = None

    def status(self):
        try:
            peq.verified_dll()
            dll_available, dll_error = True, None
        except Exception as error:
            dll_available, dll_error = False, str(error)
        return dict(token=self.token, dll_available=dll_available, dll_error=dll_error,
                    live_enabled=self.enable_live, device=self.device,
                    backup_exists=self.backup is not None, live_state=self.live_state,
                    backup_path=self.backup_path, hardware_audio_verified=False)

    def perform(self, path, request):
        if path == '/api/compile':
            return worker('peq.py', request)
        if path not in ('/api/read_device', '/api/apply', '/api/restore'):
            raise ValueError('Unknown endpoint.')
        if not self.enable_live:
            raise ValueError('Device controls are disabled. Restart with --enable-live for a deliberate headset test.')
        if path == '/api/read_device':
            if self.live_state != 'unchanged':
                raise ValueError('Restore the existing backup before reading a new one.')
            state = worker('device_worker.py', dict(operation='read'))
            self.device = f"Original Maxwell PS · USB-C · preset {state['index']}"
            if state['slot'] is None:
                self.backup = None
                self.backup_path = None
                return dict(state=state, backup_exists=False,
                            message=f"Headset read successfully. Preset {state['index']} is built-in. Choose a custom preset in the Audeze app, then read again.")
            blob = bytes.fromhex(state['saved_coefficients_hex'])
            stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S.%fZ')
            directory = ROOT / 'backups'
            directory.mkdir(exist_ok=True)
            target = directory / f'{stamp}-slot{state["slot"] + 1}.json'
            backup = dict(created_at=stamp, state=state, coefficient_sha256=hashlib.sha256(blob).hexdigest(),
                          note='Exact saved coefficients. Existing unsaved live DSP state cannot be read.')
            with target.open('x', encoding='utf-8') as output:
                json.dump(backup, output, indent=2)
            self.backup, self.backup_path = backup, str(target)
            return dict(state=state, backup_exists=True, backup_path=str(target),
                        message=f"Custom preset {state['slot'] + 1} backed up. Restore will reapply its exact saved coefficients.")
        if self.backup is None:
            raise ValueError('Read and back up the active custom preset first.')
        if path == '/api/apply':
            if self.live_state == 'uncertain':
                raise ValueError('Restore after the previous uncertain write before applying a new filter.')
            compiled = worker('peq.py', request)
            blob_hex = compiled['blob_hex']
        else:
            blob_hex = self.backup['state']['saved_coefficients_hex']
        previous = self.live_state
        self.live_state = 'uncertain'
        try:
            result = worker('device_worker.py', dict(operation='apply', blob_hex=blob_hex,
                                                    expected=self.backup['state']))
        except WorkerError as error:
            if error.result.get('write_started') is False:
                self.live_state = previous
                if error.result.get('state_changed'):
                    # An externally changed device/preset invalidates this session's snapshot.
                    self.backup = None
                    self.backup_path = None
                    self.live_state = 'unchanged'
            raise
        self.live_state = 'applied' if path == '/api/apply' else 'unchanged'
        result['message'] = ('Headset acknowledged the temporary EQ. Saved presets are unchanged; acoustic response has not been measured.'
                             if path == '/api/apply' else 'Headset acknowledged restoration of the exact saved coefficients.')
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
    server.app = App(args.enable_live)
    print(f'Editor: http://127.0.0.1:{server.server_port} | Device controls: {args.enable_live}', flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == '__main__':
    main()
