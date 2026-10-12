# Local editor API

The server binds to `127.0.0.1:8769`. GET `/api/status` returns a per-process `token`; send it as `X-Session-Token` on every POST, with `Content-Type: application/json`. All POST bodies must be JSON objects under 64 KB. Errors return HTTP 400 with `{ "error": "message" }`; authentication/host errors use 403, concurrent operations 409. Device operations require `--enable-live`. Profile operations work offline and never call the DLL or headset.

## Named local profiles

GET `/api/profiles` returns `{ "profiles": [...] }`, sorted by name. Each profile contains:

```json
{
  "id": "server-generated-identifier",
  "name": "Music",
  "revision": 1,
  "updated_at": "2026-10-12T22:00:00+00:00",
  "preset": {
    "filters": [{"type": "peak", "frequency": 1000, "gain": -3, "q": 1, "enabled": true}],
    "preamp": "auto"
  }
}
```

| POST endpoint | Body | Result |
| --- | --- | --- |
| `/api/profiles/save` | `{ "name": "Music", "preset": { ... } }` | Create a profile |
| `/api/profiles/save` | `{ "id": "...", "revision": 1, "name": "Music", "preset": { ... } }` | Update a profile |
| `/api/profiles/rename` | `{ "id": "...", "revision": 1, "name": "Evening" }` | Rename, retaining filters |
| `/api/profiles/delete` | `{ "id": "...", "revision": 1 }` | Delete from the local library |

Mutations return `{ "profile": {...}, "profiles": [...] }`; deletion returns `profile: null`. Names are trimmed, 1–80 characters, unique ignoring case, and never used as filesystem paths. The library allows 200 profiles. Update, rename and delete require the revision last loaded by that editor. Stale revisions are rejected without modification; refresh the list and explicitly reload before retrying so unsaved edits are not silently overwritten.

Filters are normalized and validated without compilation. Auto preamp remains `"auto"`; numeric manual preamp remains numeric. Compilation may still reject a profile for fixed-point precision. Library writes are atomic; device recovery remains available if the library cannot be read. `/api/status.profiles_error` reports that condition and profile endpoints return an error without overwriting the damaged file.

The library is `profiles/library.json` next to `server.py`; back it up separately from `backups/session.json`. Both directories are ignored by git.

## Editor draft

The current frontend stores a versioned draft under localStorage key `maxwell-peq-editor-v1`. It contains filters, preamp mode/value, profile name and the loaded profile revision. It permits incomplete number fields so reloads preserve work in progress. Profile version checks still apply after draft recovery.

If no valid draft exists and the user has not begun editing, `/api/status.last_profile` supplies the last applied filter JSON. This is not a read of the headset's live DSP, and may differ from current sound after a restore or external preset change. Initialization and status refresh must never automatically Apply or Save. Loading profiles and recovering drafts invalidate compiled previews.

## Connection verification

POST `/api/connect` with `{ "transport": "usb" }` or `{ "transport": "dongle" }`.

Without a session backup, it performs the ordinary read-and-backup operation. With an existing backup, it reads the selected route, compares `index`, `slot`, `raw_ui_hex` and `saved_coefficients_hex` to the expected saved snapshot, then updates only the connection, device label and expected snapshot. It preserves the original backup, backup path, last applied filters, and live/storage states. A matching read may also update a changed HID path on the same route. It does not send live or persistent EQ.

On success: `{ "state": {...}, "backup_exists": true, "backup_path": "...", "message": "..." }`. Fetch `/api/status` afterward. Mismatch, disconnect or failure to persist the new session leaves the old connection and backup intact. An uncertain storage write blocks switching; recover through the original connection. This is a comparison of preset contents, not serial-number verification.

The selector should remain usable while a profile is applied or saved. Disable Apply/Save/Restore whenever the selected connection differs from `/api/status.transport`, until connection verification succeeds. Keep ordinary writes disabled for uncertain state; a connection read must not clear that uncertainty.

Existing endpoints remain `/api/compile`, `/api/read_device`, `/api/apply`, `/api/save`, `/api/restore` (live only), and `/api/restore_saved` (saved plus live). `/api/read_device` creates a new original backup and therefore still requires an unchanged session; use `/api/connect` when preserving an existing one.
