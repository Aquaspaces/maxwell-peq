# Maxwell PEQ

Desktop editor for up to ten filters on the **original Maxwell PlayStation**, connected by USB-C or the PlayStation wireless dongle. Supports peaks, low/high shelves and low/high-pass filters, with a draggable response graph, named profiles, Equalizer APO import/export, automatic preamp headroom, live apply, persistent headset saves with exact readback, restoration from the original saved coefficient bytes, and left/right channel balance. It runs on Windows and macOS.

The connected headset's saved custom preset was read and reproduced **byte for byte** across all four sample-rate banks. On 2026-10-12, the user confirmed audible live filtering, changes when adjusting Q, and attenuation of a 1 kHz tone with a narrow -12 dB peak. Exact acoustic response has not been measured. See [PROOF.md](PROOF.md) for the evidence and limits.

## Download

Get the latest build from [Releases](https://github.com/Aquaspaces/maxwell-peq/releases):

- **Windows:** the `-setup.exe` installer, or the bare `MaxwellPEQ.exe`, which runs from any folder without installing.
- **macOS 11+:** the universal `.dmg` (Apple silicon and Intel). It isn't signed by Apple, so the first launch is blocked: open System Settings → Privacy & Security and choose **Open Anyway**.

## Start

Run `MaxwellPEQ.exe`. It opens its own window (WebView2, built into Windows 10 and 11) and needs nothing else installed. Options:

- `--offline`: edit and export without ever opening the headset.
- `--serve [--port N]`: serve the editor at `http://127.0.0.1:8769/` for a normal browser instead of opening a window.

When the Audeze app is installed (`C:\Program Files\Audeze\AudezeApp`), the app builds peak and pass filters with its `AirohaPeqLibrary.dll`; the Audeze app itself must not be running. The DLL is never bundled. It is checked against the inspected SHA-256 before use; set `MAXWELL_PEQ_DLL` to use another copy, or place the DLL next to the exe. Without it, the built-in encoder is used instead (see [Coefficient paths](#coefficient-paths)).

Profiles, backups and write records are kept in `%LOCALAPPDATA%\MaxwellPEQ` (set `MAXWELL_PEQ_DATA` to move them). Keep the `backups` folder: it holds the original preset used by Restore.

### Build

The app is Rust with [Tauri 2](https://tauri.app); the editor in `web/` is plain HTML, CSS and JavaScript with no build step. With [Rust](https://rustup.rs) 1.89 or newer and the Tauri CLI (`cargo install tauri-cli --version "^2"`):

```powershell
cd src-tauri
cargo tauri build --no-bundle   # target\release\MaxwellPEQ.exe, a single file
cargo tauri build               # also an NSIS installer under target\release\bundle
cargo run -p maxwell-peq -- --serve --offline   # debug build, editor in a browser
```

On macOS the same commands build `Maxwell PEQ.app` and a `.dmg`; `--target universal-apple-darwin` builds both architectures in one (add both targets with `rustup target add aarch64-apple-darwin x86_64-apple-darwin`).

The workspace has two crates: `crates/maxwell-core` (filter compiler, APO conversion, profiles, HID protocol, session recovery; no UI) and `src-tauri` (the window, plus the `--serve` and `--worker device` entry points).

### Releases

`.github/workflows/release.yml` runs when a `v*` tag is pushed. The tag must match the workspace version in `Cargo.toml`. It runs the tests, then builds the Windows installer, the standalone exe and the universal macOS `.dmg`, and uploads them to a **draft** release, which you publish by hand after checking it:

```powershell
# bump version in Cargo.toml, commit, then:
git tag v0.2.0
git push origin v0.2.0
```

`.github/workflows/ci.yml` runs formatting, Clippy and the tests on Windows and macOS for every pull request and every push to `main`.

### Python reference implementation

The original Python version (`server.py`, `peq.py`, `device_worker.py` and friends) stays in the repository as the reference the Rust port is checked against. `python tools/make_golden.py` regenerates the fixtures in `crates/maxwell-core/tests/golden/`, and the Rust tests require exact floating-point equality with them. `python server.py` still serves the same editor at `http://127.0.0.1:8769` with no pip packages; source runs keep profiles and backups in the repository folder unless `MAXWELL_PEQ_DATA` is set. `./start.ps1` starts it in the background.

## Audeze App/HQ must be closed

The headset's control interface can only have one reader; the Audeze app and this editor would consume each other's replies. While the Audeze app is running, the editor shows "Audeze App/HQ is open. Custom PEQ will not work if it is open, would you like to close it?" **Yes** closes it, including its tray process. **No** keeps it open and pauses every headset action until it is gone; the device bar keeps a **Close it…** link. The check runs again before every headset operation.

## Profiles

The sidebar holds named profiles. Click one, or press **Ctrl+1** to **Ctrl+9**, to switch. With **Apply live** on, switching plays the profile on the headset immediately, and every edit is sent as you make it. That change is temporary: the headset forgets it when it reloads a preset. **Save to headset…** writes the current profile into flash.

Profiles save automatically to the computer. Each one has its own undo history (**Ctrl+Z**, **Ctrl+Y**). Use the **⋯** menu to rename, duplicate, export or delete one.

## Equalizer APO

**Import Equalizer APO preset** (or **Ctrl+O**, or dropping a `.txt` file on the window) accepts the parametric text that Equalizer APO, AutoEQ and Peace export:

```
Preamp: -6.2 dB
Filter 1: ON LSC Fc 105 Hz Gain 6.0 dB Q 0.70
Filter 2: ON PK Fc 180 Hz Gain -2.4 dB Q 1.10
```

`PK`/`PEQ`/`Modal`, `LS`/`LSC`, `HS`/`HSC`, `LP`/`LPQ` and `HP`/`HPQ` filters import, and so do `BW Oct` widths. The preview lists anything that changes: band-pass, notch and all-pass filters, corner-frequency shelves and GraphicEQ lines are skipped; shelf Q becomes the headset's fixed S=1 slope; values outside the headset's range are clamped; only the first ten filters are kept. The file's preamp is replaced by auto preamp, so boosts cannot clip. The older filter JSON format imports the same way.

**Copy as APO** and **Export APO…** write the current profile back out in the same format, with the preamp the headset will actually use.

## Use the headset

1. Connect the original PS headset by USB-C, or connect its PlayStation dongle with the headset powered on and paired. Choose the matching connection in the device bar.
2. In the Audeze app, select one of the four custom presets, then close the app (the editor offers to).
3. Click **Connect & back up**. It saves the exact saved coefficients to the backups folder. Built-in presets are readable, but applying requires a custom preset.
4. Edit. With **Apply live** on, each change plays as soon as it compiles; turn it off to use the **Apply** button instead. Start at low listening volume. An acknowledgement means the headset accepted the command; it is not an audio measurement.
5. **Restore → Restore live EQ** plays the original preset again without changing headset storage.

Temporary changes may be replaced when the device or official app reloads a preset. Backups capture saved coefficients, not any pre-existing unsaved live DSP state. The app verifies the device path, active preset, UI records and saved coefficient bytes again immediately before each apply or save. Preset selection and firmware flashing are not implemented.

## Save to the headset

**Save to headset…** overwrites the active custom slot's four coefficient banks, reads them back byte for byte, then applies the same EQ live. **Restore → Restore saved preset** writes the original backup back to that slot, verifies it, and applies it. The sidebar marks the profile that is playing (**Live**) and the one in flash (**Saved**).

The Audeze app's ten slider records are left unchanged because they cannot describe these custom filters. Its sliders will show old values, and moving them can replace the saved EQ. The raw backup preserves the exact original sound but cannot reconstruct arbitrary filter settings; keep profiles or APO exports for editable copies.

The app journals each write before sending it and recovers the session, connection and original backup after restart. If a write times out, further Apply/Save operations are blocked until restoration. Saved-profile recovery permits replacing an uncertain stored blob, while still checking the interface, custom slot and UI records. After sending EQ you can still switch between USB-C and the dongle: choose the connection and click **Read again**. The app reads the headset through it and checks that the preset number, slider records and saved coefficients match what it expects. If they do, it keeps the original backup and the live and saved states, and sends no EQ; if not, nothing changes. A save with an uncertain result has to be restored through its original connection first. Taking a new backup requires restoring the existing session.

Hardware testing on 2026-10-12 confirmed a save to Custom 4 through the dongle, exact 844-byte readback after powering the headset off/on with USB unplugged, and user-reported audible EQ retention during wireless playback. See [PROOF.md](PROOF.md). [examples/obvious-persistence.json](examples/obvious-persistence.json) contains the deliberately exaggerated two-cut test profile.

## Channel balance

**Balance…** in the device bar sets the left and right driver levels, for headsets whose channels don't match. The dialog reads both levels through the selected connection when it opens (**Read headset** reads again), and the app keeps the first reading so **Put back** can always return to it. Change a side by one to three steps at a time and listen; higher values are louder. The headset applies each write immediately.

- Values run from 0 to 150. 150 is a hard ceiling: higher values can damage the drivers, so the app never sends them. 136 and 142 are skipped because the firmware reserves them.
- Factory values on current firmware are 141 / 149 on the USB-C profile and 147 / 147 on the wireless profile.
- Balance is saved on the headset, but the headset only loads the saved values at power-on when its source profile is USB-C. On the wireless profile it loads its wireless defaults instead, so the dialog offers to switch the profile. The switch takes effect at the next power-on.
- A factory reset or a fully drained battery returns the factory values.

Balance writes are limited to the two level settings and the source profile, and each value is read back twice before it counts. On 2026-10-12 a write of 147/144 over USB-C from a Mac was heard leaning left, and 147/147 was then written back and read back.

The balance protocol, value limits and persistence behaviour come from kats1123's [maxwell-firmware-research](https://github.com/kats1123/maxwell-firmware-research) (CC BY 4.0) and [maxwell-balance-and-firmware-tool](https://github.com/kats1123/maxwell-balance-and-firmware-tool) (MIT). No code, DLLs or firmware files from those repositories are included.

## macOS and other systems

The macOS build reaches the headset through [hidapi](https://github.com/ruabmbua/hidapi-rs) and reads report sizes from the HID report descriptor (`crates/maxwell-core/src/descriptor.rs`). It does everything the Windows build does. The Audeze DLL only exists on Windows, so the Mac always uses the built-in encoder. The Audeze app check looks for processes named `Audeze`, `Audeze HQ` and `AudezeHQ`. On 2026-10-12, on an M1 Pro Mac over USB-C, it read and backed up the headset, applied built-in-encoder peaks and stacked high- and low-pass filters that were clearly audible, restored the backup, and changed balance and put it back. It then switched to the PlayStation dongle, read the headset again, and saved a flat profile to headset storage, which read back exactly. The app window opens and shows the same state.

Profiles live in `~/Library/Application Support/MaxwellPEQ` on macOS and `$XDG_DATA_HOME/maxwell-peq` on Linux. Linux builds compile (hidapi needs `libudev-dev`) but aren't packaged.

## Supported values and limits

- 1-10 filters of any supported type; 20-20,000 Hz. Disabled rows are replaced with unity filters.
- Peaks: -12 to +12 dB, Q 0.1-20. This path has user listening confirmation.
- Low/high shelves: -12 to +12 dB, fixed monotonic slope S=1 (Q approximately 0.7071). Frequency sets the shelf midpoint. Shelf slope is not adjustable in this version.
- Low/high-pass: second order (12 dB/octave asymptotic slope), Q 0.1-20. Gain is unused and must be 0. Q approximately 0.7071 gives the Butterworth response; larger Q adds resonance near the cutoff.
- **Shelf and pass filters are experimental:** numerical checks pass, but they have not yet been confirmed by listening on the headset. The type selector and compiled result identify them.
- Auto preamp subtracts the sum of each filter's maximum boost, including pass-filter resonance. Manual preamp must provide at least that much headroom; maximum attenuation is 60 dB.
- Compilation checks pole stability and compares decoded quantized response against the RBJ reference. It rejects sampled errors over 0.15 dB. For shelf/pass sets, comparisons floor both curves at -90 dB to handle deep stopband nulls and also check DC/Nyquist and pass-filter resonance. Some low-frequency or high-Q combinations exceed the precision limit; the UI reports the failure instead of sending them.
- Banks: 44.1, 48, 88.2 and 96 kHz. Firmware version is not identified; USB device revision is not treated as firmware version.
- The window talks to the app over Tauri IPC; `--serve` is loopback only with a per-process request token and same-origin checks. One device operation runs at a time. No telemetry or external assets.
- Only one copy of the desktop app runs at a time, since two would share the headset interface.

## Validate

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets
cargo clippy -p maxwell-core --features hidapi --all-targets   # the macOS HID backend, on Windows
node --check web/app.js
node --check web/graph.js
python -m unittest discover -s tests   # the Python reference
```

The Rust tests port every Python test and add golden parity checks: compiled coefficient blobs, curves, preamps and validation errors must equal the Python output exactly. Tests cover the vendor math against reference responses, Q width, coefficient packing, packet allowlists, fragmented HID messages, malformed replies, storage capacity/acknowledgement parsing, exact readback, recovery across server restarts and uncertain writes, Equalizer APO import/export, the profile library, and the Audeze app guard. Tests never send hardware writes. Vendor tests need the inspected DLL; other tests run without it.

Native HID operations run in a bounded child process (`MaxwellPEQ.exe --worker device`, JSON on stdin and stdout) because a native driver can stall; a timed-out worker is killed and the session is marked uncertain. `peq.py` accepts a preset JSON object on stdin and emits compiled JSON on stdout; see `examples/one-peak.json`.

## Filter JSON

Each row accepts `type`: `peak`, `low_shelf`, `high_shelf`, `low_pass`, or `high_pass`. Missing type and legacy `PK` mean peak. Shelves may omit Q; pass filters may omit gain and Q. See [examples/shelf-and-pass.json](examples/shelf-and-pass.json). Importing/exporting retains filter types; these files are stored on the computer, not saved into headset flash.

## Coefficient paths

Peak/pass-only sets use the inspected vendor DLL when it is installed, including its native `set_xpf_point` function. Any enabled shelf switches the whole cascade to an RBJ encoder using the same verified 24-bit numeric layout. That encoder bounds individual stage gain, retains the vendor's first-stage attenuation margin, and compensates in the global scale. All four sample-rate banks are checked after encoding. The original peak-only example remains byte-identical to the listening-tested build.

Without the DLL (on macOS, or on Windows without the Audeze app), peak/pass-only sets use the built-in encoder too. Its peak stages copy the vendor's layout: each numerator is scaled so b0 is unity, and the last numerator coefficient is rounded so the stage's DC gain stays exact on the 24-bit grid. Without that, low narrow peaks at 88.2 and 96 kHz lose up to 0.4 dB. `tests/encoder.rs` compiles 400 random peak sets both ways. The built-in encoder builds every set the DLL builds, with responses within 0.21 dB of the DLL's, and builds 91 sets the DLL rejects for precision. A −12 dB peak it built was clearly audible on a Mac, but it hasn't been compared with the DLL by ear. To compare the two by ear on Windows, start the app with `MAXWELL_PEQ_ENCODER=built-in`, which uses the built-in encoder even when the DLL is installed. The editor says which encoder is active.

Dongle requests use the headset route flag `0x80`. Persistent writes are restricted to the four custom coefficient keys `E42C` through `E42F`; arbitrary storage writes, erase commands and firmware operations are excluded.

## License

Maxwell PEQ is free software under the [GNU General Public License v3.0](LICENSE) or any later version. You can use, study, change and share it. If you distribute a modified version, you must release its source under the same license.

The Audeze app and its `AirohaPeqLibrary.dll` are Audeze's and are not covered by this license; Maxwell PEQ never includes them. Maxwell PEQ is not made or endorsed by Audeze.
