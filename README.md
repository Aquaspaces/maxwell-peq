# Maxwell PEQ proof of concept

Local Windows editor for up to ten peaking filters on the **original Maxwell PlayStation**, connected directly by USB-C. Each filter has an arbitrary frequency, gain and Q. Includes calculated response plots, automatic preamp headroom, JSON import/export, temporary apply, and restoration from exact saved coefficient bytes.

The connected headset's saved custom preset was read and reproduced **byte for byte** across all four sample-rate banks. On 2026-10-12, the user confirmed audible live filtering, changes when adjusting Q, and attenuation of a 1 kHz tone with a narrow -12 dB peak. Exact acoustic response has not been measured. See [PROOF.md](PROOF.md) for the evidence and limits.

## Start

Requires 64-bit Windows Python and the inspected Audeze math DLL. No pip packages are required. The DLL is not redistributed; this machine uses `X:/audeze-re/extract/audeze-app/AirohaPeqLibrary.dll`. Set `MAXWELL_PEQ_DLL` to another copy with the same verified SHA-256 if needed.

```powershell
cd X:/maxwell-peq-poc
python server.py --enable-live
```

Open http://127.0.0.1:8769. Omit `--enable-live` for compilation and preview only. Use `./start.ps1` to start a hidden background server; it prints the process ID and log location without opening a browser.

## Try a temporary filter

1. Connect the original PS headset directly with USB-C. The dongle is not supported by this build.
2. In Audeze HQ, select one of the four custom presets. Fully quit HQ, including its tray process. The editor requires exclusive access to the headset control interface because concurrent readers consume each other's response fragments.
3. In the local editor, click **Read & back up preset**. It saves exact coefficients under `backups/`. Built-in presets are readable, but applying requires a custom preset.
4. Set frequency, gain and Q, then **Compile preview**. The default is a gentle 1 kHz, -3 dB, Q 1 peak. The four curves show the decoded fixed-point coefficients, including preamp.
5. At low listening volume, click **Apply temporarily**. An acknowledgement means the headset accepted the command; it is not an audio measurement.
6. Click **Restore backup** to reapply the original saved coefficients. Reselecting the saved preset in HQ also provides a recovery path if the server was closed.

No preset-selection, persistent EQ write, or firmware-flash command is implemented. Temporary changes may be replaced when the device or official app reloads a preset. Backups capture saved coefficients, not any pre-existing unsaved live DSP state. The app verifies the device path, active preset, UI records and saved coefficient bytes again immediately before each apply or restore. Avoid changing presets in HQ during a test.

If a write times out, the editor blocks further Apply operations and offers Restore. A confirmed pre-send preset change clears the stale backup so a fresh read is possible. Backups survive a restart on disk, but this version does not import them into a new session: reselect the saved preset in HQ before beginning another session.

## Supported values and limits

- 1-10 peak filters; 20-20,000 Hz; -12 to +12 dB; Q 0.1-20. Disabled rows are replaced with unity filters.
- High/low-pass and shelf filters are not implemented.
- Auto preamp subtracts the sum of positive gains. Manual preamp must provide at least that much headroom; maximum attenuation is 60 dB.
- Compilation checks pole stability and compares quantized response against an independent RBJ reference. It rejects sampled errors over 0.15 dB. Some low-frequency or high-Q combinations exceed this limit; the UI reports the failure instead of sending them.
- Banks: 44.1, 48, 88.2 and 96 kHz. Firmware version is not identified; USB device revision is not treated as firmware version.
- Loopback only, per-process request token, same-origin checks, and one serialized device operation. No telemetry or external assets.

## Validate

```powershell
python -m unittest discover -s tests -v
node --check web/app.js
```

Tests cover the vendor math against reference responses, Q width, coefficient packing, packet allowlists, fragmented HID messages, malformed replies and recovery state transitions. Tests never send hardware writes. Vendor tests need the inspected DLL; other tests run without it.

`peq.py` also accepts a preset JSON object on stdin and emits compiled JSON on stdout. See `examples/one-peak.json`. Compilation and native HID operations run in bounded subprocesses because a native driver or DLL can stall.
