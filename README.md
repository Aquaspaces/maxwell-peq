# Maxwell PEQ proof of concept

Local Windows editor for up to ten filters on the **original Maxwell PlayStation**, connected by USB-C or the PlayStation wireless dongle. Supports peaks, low/high shelves and low/high-pass filters. Includes calculated response plots, automatic preamp headroom, JSON import/export, temporary apply, persistent headset saves with exact readback, and restoration from the original saved coefficient bytes.

The connected headset's saved custom preset was read and reproduced **byte for byte** across all four sample-rate banks. On 2026-10-12, the user confirmed audible live filtering, changes when adjusting Q, and attenuation of a 1 kHz tone with a narrow -12 dB peak. Exact acoustic response has not been measured. See [PROOF.md](PROOF.md) for the evidence and limits.

## Start

Requires 64-bit Windows Python and the inspected Audeze math DLL. No pip packages are required. The DLL is not redistributed; this machine uses `X:/audeze-re/extract/audeze-app/AirohaPeqLibrary.dll`. Set `MAXWELL_PEQ_DLL` to another copy with the same verified SHA-256 if needed.

```powershell
cd X:/maxwell-peq-poc
python server.py --enable-live
```

Open http://127.0.0.1:8769. Omit `--enable-live` for compilation and preview only. Use `./start.ps1` to start a hidden background server; it prints the process ID and log location without opening a browser.

## Try a temporary filter

1. Connect the original PS headset by USB-C, or connect its PlayStation dongle with the headset powered on and paired. Choose the matching connection in the editor.
2. In Audeze HQ, select one of the four custom presets. Fully quit HQ, including its tray process. The editor requires exclusive access to the headset control interface because concurrent readers consume each other's response fragments.
3. In the local editor, click **Read & back up preset**. It saves exact coefficients under `backups/`. Built-in presets are readable, but applying requires a custom preset.
4. Set frequency, gain and Q, then **Compile preview**. The default is a gentle 1 kHz, -3 dB, Q 1 peak. The four curves show the decoded fixed-point coefficients, including preamp.
5. At low listening volume, click **Apply temporarily**. An acknowledgement means the headset accepted the command; it is not an audio measurement.
6. Click **Restore live backup** to reapply the original saved coefficients without changing headset storage.

Temporary changes may be replaced when the device or official app reloads a preset. Backups capture saved coefficients, not any pre-existing unsaved live DSP state. The app verifies the device path, active preset, UI records and saved coefficient bytes again immediately before each apply or save. Avoid changing presets in HQ during a test. Preset selection and firmware flashing are not implemented.

## Save a persistent profile

After reading a backup and compiling, click **Save to headset**. This overwrites the active custom slot's four coefficient banks, reads them back byte for byte, then applies the same EQ live. **Restore saved profile** writes the original backup back to that slot, verifies it, and applies it. **Restore live backup** only changes the current sound; a saved custom EQ remains in storage.

HQ's ten slider records are left unchanged because they cannot describe these custom filters. Its sliders will show old values, and editing them can replace the saved EQ. Keep filter JSON exports for editable profiles; the raw backup preserves the exact original sound but cannot reconstruct arbitrary filter settings.

The server journals each write before sending it and recovers the session, connection and original backup after restart. If a write times out, further Apply/Save operations are blocked until restoration. Saved-profile recovery permits replacing an uncertain stored blob, while still checking the interface, custom slot and UI records. Taking a new backup requires restoring the existing session. All backups and write records stay under ignored `backups/`; keep that directory for recovery.

## Local profiles and editor recovery

Use **Save as new** to name the current filters, **Load** to open a profile, **Update** to replace its saved settings, or **Rename** / **Delete** to manage it. These actions only affect files on the computer. Apply and Save to headset remain separate. Loading discards the current editor contents, so save a new profile or update the current one first if you want to keep those edits.

The local library lives in ignored `profiles/library.json`. Names are labels, not filenames. Saving does not require the vendor DLL or a connected headset; compilation is still required before applying. Stale edits from another browser tab cannot overwrite a newer saved profile without reloading it. JSON import/export remains available for transferring individual profiles.

The browser keeps the current editor draft, including unsaved filter edits and the selected profile, across reloads. With no browser draft, the editor reopens the last filters applied through this server. Neither recovery path automatically sends EQ. Drafts belong to the browser and local server address; use named profiles or exports to keep settings across browsers or computers. Clearing browser storage removes drafts but leaves the file-based library intact.

## Change USB / dongle connections

Choose a connection and click **Connect / verify**. With an existing backup, this reads the selected interface and compares the current custom slot, slider records and exact saved coefficient bytes with the session's expected state. If they match, the server updates the control interface while retaining the original backup and live/saved state. No restore, save or live EQ is sent. The same action can rebind a connection after Windows changes its HID path.

If the preset differs or the connection is unavailable, the existing session remains intact. An uncertain persistent write must be recovered through its original connection first. This comparison establishes matching preset contents, not a unique physical headset serial number. Only the original PS model and its PS dongle are supported.

See [API.md](API.md) for the profile and connection endpoints used by the interface.

Hardware testing on 2026-10-12 confirmed a save to Custom 4 through the dongle, exact 844-byte readback after powering the headset off/on with USB unplugged, and user-reported audible EQ retention during wireless playback. See [PROOF.md](PROOF.md). [examples/obvious-persistence.json](examples/obvious-persistence.json) contains the deliberately exaggerated two-cut test profile.

## Supported values and limits

- 1-10 filters of any supported type; 20-20,000 Hz. Disabled rows are replaced with unity filters.
- Peaks: -12 to +12 dB, Q 0.1-20. This path has user listening confirmation.
- Low/high shelves: -12 to +12 dB, fixed monotonic slope S=1 (Q approximately 0.7071). Frequency sets the shelf midpoint. Shelf slope is not adjustable in this version.
- Low/high-pass: second order (12 dB/octave asymptotic slope), Q 0.1-20. Gain is unused and must be 0. Q approximately 0.7071 gives the Butterworth response; larger Q adds resonance near the cutoff.
- **Shelf and pass filters are experimental:** numerical checks pass, but they have not yet been confirmed by listening on the headset. The type selector and compiled result identify them.
- Auto preamp subtracts the sum of each filter's maximum boost, including pass-filter resonance. Manual preamp must provide at least that much headroom; maximum attenuation is 60 dB.
- Compilation checks pole stability and compares decoded quantized response against the RBJ reference. It rejects sampled errors over 0.15 dB. For shelf/pass sets, comparisons floor both curves at -90 dB to handle deep stopband nulls and also check DC/Nyquist and pass-filter resonance. Some low-frequency or high-Q combinations exceed the precision limit; the UI reports the failure instead of sending them.
- Banks: 44.1, 48, 88.2 and 96 kHz. Firmware version is not identified; USB device revision is not treated as firmware version.
- Loopback only, per-process request token, same-origin checks, and one serialized device operation. No telemetry or external assets.

## Validate

```powershell
python -m unittest discover -s tests -v
node --check web/app.js
```

Tests cover the vendor math against reference responses, Q width, coefficient packing, packet allowlists, fragmented HID messages, malformed replies, storage capacity/acknowledgement parsing, exact readback, and recovery across server restarts and uncertain writes. Tests never send hardware writes. Vendor tests need the inspected DLL; other tests run without it.

`peq.py` also accepts a preset JSON object on stdin and emits compiled JSON on stdout. See `examples/one-peak.json`. Compilation and native HID operations run in bounded subprocesses because a native driver or DLL can stall.

## Filter JSON

Each row accepts `type`: `peak`, `low_shelf`, `high_shelf`, `low_pass`, or `high_pass`. Missing type and legacy `PK` mean peak. Shelves may omit Q; pass filters may omit gain and Q. See [examples/shelf-and-pass.json](examples/shelf-and-pass.json). Importing/exporting retains filter types; these files are stored on the computer, not saved into headset flash.

## Coefficient paths

Peak/pass-only sets use the inspected vendor DLL, including its native `set_xpf_point` function. Any enabled shelf switches the whole cascade to an RBJ encoder using the same verified 24-bit numeric layout. That encoder bounds individual stage gain, retains the vendor's first-stage attenuation margin, and compensates in the global scale. All four sample-rate banks are checked after encoding. The original peak-only example remains byte-identical to the listening-tested build.

Dongle requests use the headset route flag `0x80`. Persistent writes are restricted to the four custom coefficient keys `E42C` through `E42F`; arbitrary storage writes, erase commands and firmware operations are excluded.
