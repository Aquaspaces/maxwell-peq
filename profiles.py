"""Local editable profiles. No DLL calls or device access."""
import copy
from datetime import datetime, timezone
import json
from pathlib import Path
import secrets

import peq


def preset(value):
    filters, preamp = peq.validate(value)
    return dict(filters=filters, preamp='auto' if value.get('preamp', 'auto') == 'auto' else preamp)


def name(value):
    if not isinstance(value, str) or not value.strip() or len(value.strip()) > 80:
        raise ValueError('Use a profile name between 1 and 80 characters.')
    if any(ord(c) < 32 for c in value):
        raise ValueError('Profile names cannot contain control characters.')
    return value.strip()


class ProfileStore:
    def __init__(self, path, write_json):
        self.path = Path(path) if path else None
        self.write_json = write_json
        self.items = []
        if self.path and self.path.exists():
            data = json.loads(self.path.read_text(encoding='utf-8'))
            if not isinstance(data, dict) or data.get('version') != 1 or not isinstance(data.get('profiles'), list):
                raise ValueError('Invalid profile library; preserve profiles/library.json for recovery.')
            ids, names = set(), set()
            for item in data['profiles']:
                if (not isinstance(item, dict) or not isinstance(item.get('id'), str)
                        or item['id'] in ids or type(item.get('revision')) is not int or item['revision'] < 1):
                    raise ValueError('Invalid profile entry in library.')
                title = name(item['name'])
                if title.casefold() in names:
                    raise ValueError('Duplicate profile name in library.')
                ids.add(item['id'])
                names.add(title.casefold())
                self.items.append(dict(item, name=title, preset=preset(item['preset'])))

    def list(self):
        return copy.deepcopy(sorted(self.items, key=lambda item: item['name'].casefold()))

    def change(self, operation, request):
        items = copy.deepcopy(self.items)
        identifier = request.get('id')
        existing = next((p for p in items if p['id'] == identifier), None)
        if identifier is not None:
            if existing is None:
                raise ValueError('This profile no longer exists. Reload the profile list.')
            if type(request.get('revision')) is not int or request['revision'] != existing['revision']:
                raise ValueError('This profile changed in another editor. Reload it before changing it.')
        elif operation != 'save':
            raise ValueError('Choose a saved profile first.')
        if operation == 'delete':
            items.remove(existing)
            result = None
        elif operation in ('save', 'rename'):
            title = name(request.get('name'))
            if any(p['id'] != identifier and p['name'].casefold() == title.casefold() for p in items):
                raise ValueError('A profile already uses this name. Choose a different name or update that profile.')
            if existing is None:
                if len(items) >= 200:
                    raise ValueError('The library is full (200 profiles). Export and remove an unused profile first.')
                existing = dict(id=secrets.token_hex(16), revision=0)
                items.append(existing)
            existing.update(name=title, revision=existing['revision'] + 1,
                            updated_at=datetime.now(timezone.utc).isoformat(),
                            preset=preset(request['preset']) if operation == 'save' else existing['preset'])
            result = existing
        else:
            raise ValueError('Unknown profile operation.')
        if self.path:
            self.write_json(self.path, dict(version=1, profiles=items))
        self.items = items
        return dict(profile=copy.deepcopy(result), profiles=self.list())
