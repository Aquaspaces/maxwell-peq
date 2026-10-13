# Maxwell PEQ

A parametric EQ editor for the Audeze Maxwell, on Windows, macOS and Linux. It writes real peak, shelf and pass filters to the headset's four custom presets, which Audeze HQ's ten-band graphic EQ can't do.

- Up to ten filters, with a draggable response graph and automatic preamp headroom
- Named profiles, with Equalizer APO and AutoEQ import and export
- Save a profile to any of the four custom presets, read back byte for byte, and switch the active preset in one click
- Left/right channel balance and the everyday Audeze HQ controls
- Connects over USB-C or the wireless dongle

It has been tested only on the **original Maxwell PlayStation** (see [Headsets](#headsets)). The headset's own saved preset was read and reproduced byte for byte across all four sample-rate banks. On 2026-10-12 the user confirmed audible filtering, changes when adjusting Q, and attenuation of a 1 kHz tone with a narrow −12 dB peak. On 2026-10-13 a rough mic measurement matched the expected peak, shelf and pass responses within about 0.2 dB RMS from 300 Hz to 3 kHz; bass and treble are unmeasured. [PROOF.md](PROOF.md) has the evidence and its limits.

## Headsets

| Headset | USB product IDs | Status |
| --- | --- | --- |
| Original Maxwell PlayStation | `4B1A` USB-C, `4B19` dongle | Tested |
| Maxwell 2 | `4B2A` USB-C, `4B28`/`4B29` dongle | Supported but untested |
| Maxwell 2 ANC | `4BA8`–`4BAA` | Not supported |
| Original Maxwell Xbox | | Untested |

Maxwell 2 support comes only from static analysis of Audeze HQ 1.2.48. HQ sends Maxwell 2 EQ through the same save command as the original, and that is what this app sends. After a save over USB-C, HQ also reselects the active custom preset so the headset reloads it, muting voice prompts for that second; the app does the same, on Maxwell 2 over USB-C only. Nobody has run this app against a Maxwell 2 yet, so start at low volume. Channel balance and the **Headset…** controls stay limited to the original Maxwell.

HQ reads Maxwell 2 ANC EQ with a different command and data layout. What the ANC stores, and whether any of this applies to it, is unknown, so the app refuses it before sending anything.

## Download

Get the latest build from [Releases](https://github.com/Aquaspaces/maxwell-peq/releases):

- **Windows:** the `-setup.exe` installer, or the bare `MaxwellPEQ.exe`, which runs from any folder without installing. It uses WebView2, which is built into Windows 10 and 11.
- **macOS 11+:** the universal `.dmg` (Apple silicon and Intel). It isn't signed by Apple, so the first launch is blocked: open System Settings → Privacy & Security and choose **Open Anyway**.
- **Linux (x64):** the `.deb` (Debian, Ubuntu), the `.rpm` (Fedora, openSUSE) or the `.AppImage`. The `.deb` and `.rpm` install a udev rule that lets you open the headset without root; with the AppImage, install it yourself (see [Linux](#linux)).

Command-line options:

- `--offline`: edit and export without ever opening the headset.
- `--serve [--port N]`: serve the editor at `http://127.0.0.1:8769/` for a normal browser instead of opening a window.

Only one copy of the app runs at a time, since two would share the headset interface.

## Close Audeze HQ first

The headset's control interface can only have one reader, so the Audeze app and this editor would consume each other's replies. While the Audeze app is running, the editor asks whether to close it. **Yes** closes it, including its tray process. **No** keeps it open and pauses every headset action until it is gone, and the device bar keeps a **Close it…** link. The check runs again before every headset operation.

## Use the headset

1. Connect the headset by USB-C, or plug in its wireless dongle with the headset on and paired. The device bar's connection is **Automatic** by default: USB-C when the cable is plugged in, the dongle otherwise. Choose one there to force it.
2. Close the Audeze app (the editor offers to).
3. The editor reads the headset when it opens. Click **Read again** after changing presets or connections on the headset itself.
4. Drag a profile from **Not on the headset** onto the box next to **Preset 1** to **Preset 4**, or use **Save to Preset N** in its **⋯** menu. Start at low listening volume.
5. Click a preset's dot or name to make it active.

The custom presets are indexes 7 to 10, which the Audeze app shows as Preset 1 to 4. The active one has a green dot, and the device bar names it. Built-in presets can be read but not written. Firmware updates are not supported; use the Audeze app for those.

### Saving and switching

Saving overwrites the preset's four coefficient banks and reads them back byte for byte. If the preset is active, its new EQ plays right away; otherwise it plays the next time the preset becomes active. Switching presets sends only the switch, the same command the Audeze app uses, and the headset loads the preset's stored EQ itself.

Each preset remembers which profile was saved to it and shows it by name; profiles on no preset stay in the list below. If you edit a profile after saving it, its preset says **Edited since saving** with an **Update** link. The link between preset and profile is dropped when the headset reports different EQ in that preset (for example after the Audeze app rewrites it), or when a save starts but can't be verified.

The Audeze app's ten slider records are left unchanged because they can't describe these filters. Its sliders will show old values, and moving them can replace the saved EQ; reselecting the preset in the Audeze app also rewrites it from those sliders. Keep profiles or APO exports as your editable copies.

Hardware testing on 2026-10-12 confirmed a save to Custom 4 through the dongle, exact 844-byte readback after powering the headset off and on with USB unplugged, and audible EQ retention during wireless playback. See [PROOF.md](PROOF.md). [examples/obvious-persistence.json](examples/obvious-persistence.json) is the deliberately exaggerated two-cut test profile.

## Profiles

The sidebar lists named profiles. Click one, or press **Ctrl+1** to **Ctrl+9**, to edit it. Editing never touches the headset; nothing is sent until you save a profile to a preset.

Profiles save automatically to the computer, and each has its own undo history (**Ctrl+Z**, **Ctrl+Y**). The **⋯** menu renames, duplicates, exports or deletes one, or saves it to a preset.

On the graph, drag a handle to set frequency and gain, scroll over a handle to change its Q, and double-click empty space to add a filter.

## Equalizer APO

**Import Equalizer APO preset** (or **Ctrl+O**, or dropping a `.txt` file on the window) accepts the parametric text that Equalizer APO, AutoEQ and Peace export:

```
Preamp: -6.2 dB
Filter 1: ON LSC Fc 105 Hz Gain 6.0 dB Q 0.70
Filter 2: ON PK Fc 180 Hz Gain -2.4 dB Q 1.10
```

`PK`/`PEQ`/`Modal`, `LS`/`LSC`, `HS`/`HSC`, `LP`/`LPQ` and `HP`/`HPQ` filters import, and so do `BW Oct` widths. The preview lists anything that changes:

- Band-pass, notch and all-pass filters, corner-frequency shelves and GraphicEQ lines are skipped.
- Shelf Q becomes the headset's fixed S = 1 slope.
- Values outside the headset's range are clamped.
- Only the first ten filters are kept.
- The file's preamp is replaced by auto preamp, so boosts can't clip.

The older filter JSON format imports the same way. **Copy as APO** and **Export APO…** write the current profile back out in the same format, with the preamp the headset will actually use.

## Channel balance

**Balance…** in the device bar (original Maxwell only) sets the left and right driver levels, for headsets whose channels don't match. The dialog reads both levels through the selected connection when it opens, and keeps that first reading so **Put back** can always return to it. Change a side by one to three steps at a time and listen; higher values are louder. The headset applies each write immediately.

- Values run from 0 to 150. 150 is a hard ceiling: higher values can damage the drivers, so the app never sends them. 136 and 142 are skipped because the firmware reserves them.
- Factory values on current firmware are 141 / 149 on the USB-C profile and 147 / 147 on the wireless profile.
- The headset loads saved balance at power-on only when its source profile is USB-C. On the wireless profile it loads its wireless defaults instead, so the dialog offers to switch the profile, which takes effect at the next power-on.
- A factory reset or a fully drained battery returns the factory values.

Balance writes are limited to the two levels and the source profile, and each value is read back twice before it counts. On 2026-10-12 a write of 147 / 144 over USB-C from a Mac was heard leaning left, and 147 / 147 was then written and read back.

The balance protocol, value limits and persistence behaviour come from kats1123's [maxwell-firmware-research](https://github.com/kats1123/maxwell-firmware-research) (CC BY 4.0) and [maxwell-balance-and-firmware-tool](https://github.com/kats1123/maxwell-balance-and-firmware-tool) (MIT). No code, DLLs or firmware files from those repositories are included.

## Headset controls

**Headset…** in the device bar (original Maxwell only) has the settings Audeze HQ shows on its Sound and Product Settings pages: game/chat mix, volume limiter, sidetone and its level, noise suppression (off, low, high), auto shutdown (5 minutes to never) and voice prompt volume. It also shows the battery level, whether the mic is muted, and the firmware version of the headset (and of the dongle, when connected through it). Each change goes to the headset at once and is read back before it counts; if the readback doesn't match, the dialog says so and asks you to read again.

The same dialog switches between all ten onboard presets. The headset plays the new preset's own EQ, and the app reads the headset again afterwards.

These commands were decoded from Audeze HQ 1.2.48 and haven't been tested on a headset yet. Factory reset and the audio driver reinstall are left out on purpose.

## Data and the Audeze DLL

Profiles and balance write logs live in:

- Windows: `%LOCALAPPDATA%\MaxwellPEQ`
- macOS: `~/Library/Application Support/MaxwellPEQ`
- Linux: `$XDG_DATA_HOME/maxwell-peq` (usually `~/.local/share/maxwell-peq`)

Set `MAXWELL_PEQ_DATA` to move them. Older versions also kept preset backups and a `session.json` in a `backups` folder; this version ignores them, and you can delete the folder.

On Windows with the Audeze app installed (`C:\Program Files\Audeze\AudezeApp`), the app builds peak and pass filters with Audeze's `AirohaPeqLibrary.dll`. The Audeze app itself must not be running. The DLL is never bundled, and it is checked against the inspected SHA-256 before use. Set `MAXWELL_PEQ_DLL` to use another copy, or place the DLL next to the exe. Without it, the built-in encoder is used (see [Coefficient paths](#coefficient-paths)).

## macOS and Linux

Both reach the headset through [hidapi](https://github.com/ruabmbua/hidapi-rs) and read report sizes from the HID report descriptor (`crates/maxwell-core/src/descriptor.rs`). They do everything the Windows build does, always with the built-in encoder, since the Audeze DLL only exists on Windows. On macOS the Audeze app check looks for processes named `Audeze`, `Audeze HQ` and `AudezeHQ`.

On 2026-10-12, on an M1 Pro Mac over USB-C, an earlier build that applied EQ live read the headset, applied built-in-encoder peaks and stacked high- and low-pass filters that were clearly audible, and changed balance and put it back. Over the PlayStation dongle it then saved a flat profile to headset storage, which read back exactly.

### Linux

Linux uses `/dev/hidraw`. Those nodes belong to root, so the app needs the udev rule in [`src-tauri/linux/70-maxwell-peq.rules`](src-tauri/linux/70-maxwell-peq.rules), which gives the logged-in user access to the Maxwell's control interface over USB-C and the dongle. The `.deb` and `.rpm` install it to `/usr/lib/udev/rules.d` and reload udev. For the AppImage or a source build:

```sh
sudo cp src-tauri/linux/70-maxwell-peq.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger --subsystem-match=hidraw
```

Then unplug and replug the headset or dongle. Without the rule, the app says it has no permission to open the headset.

On 2026-10-12 an earlier Linux build was tested in WSL2 (Ubuntu 26.04, kernel 6.18) with the dongle passed through by [usbipd-win](https://github.com/dorssel/usbipd-win). It read Custom 4 back byte for byte (the same SHA-256 as the Windows readback), read the balance, and applied a live EQ, which the headset acknowledged with the same reply it gives on Windows. The first read after attaching failed once with a packet-boundary error and the next three passed. Saving to a preset, USB-C and the desktop window haven't been tested on Linux, and nothing has been tested on a native Linux machine.

## Supported values and limits

- 1–10 filters of any supported type, 20–20,000 Hz. Disabled rows are replaced with unity filters.
- **Peaks:** −12 to +12 dB, Q 0.1–20. Confirmed by listening on the headset.
- **Low/high shelves:** −12 to +12 dB with a fixed slope S = 1 (Q ≈ 0.7071). Frequency sets the shelf midpoint. The slope isn't adjustable.
- **Low/high-pass:** second order (12 dB/octave), Q 0.1–20, no gain. Q ≈ 0.7071 gives the Butterworth response; larger Q adds resonance near the cutoff.
- **Shelf and pass filters are experimental.** Numerical checks pass and a rough measurement on the headset matched them from 300 Hz to 3 kHz (see [PROOF.md](PROOF.md)), but their bass and treble response hasn't been measured. The type selector and compiled result mark them.
- **Auto preamp** subtracts the peak of the combined response, sampled at every rate around each filter and its pass-filter resonance, rounded up to 0.01 dB. Overlapping boosts and cuts offset each other, so it is usually far less than the sum of the boosts. If quantization still lifts the compiled response past +0.15 dB, it drops a little further. Manual preamp must provide at least the curve's peak; maximum attenuation is 60 dB.
- **Precision:** compilation checks pole stability and compares the decoded, quantized response against the RBJ reference, rejecting sampled errors over 0.5 dB (narrow sub-bass peaks come close at 88.2 and 96 kHz). For shelf and pass sets, both curves are floored at −90 dB to handle deep stopband nulls, and DC, Nyquist and pass-filter resonance are checked too. When a bank exceeds the limit, the app sends the nearest set that doesn't: at that rate it first tries the built-in encoder in place of the DLL, then raises one filter's frequency (up to an octave, lowest filters first) or lowers its Q (down to a quarter), in the smallest steps the editor shows. Other rates keep the exact values, and the editor notes each change. Only sets that no such change fixes are rejected.
- **Banks:** 44.1, 48, 88.2 and 96 kHz. The USB device revision is not treated as a firmware version.
- The window talks to the app over Tauri IPC. `--serve` listens on loopback only, with a per-process request token and same-origin checks. One device operation runs at a time. No telemetry and no external assets.

## How it works

### Coefficient paths

Peak/pass-only sets use the inspected vendor DLL when it is installed, including its native `set_xpf_point` function. Any enabled shelf switches the whole cascade to an RBJ encoder using the same verified 24-bit numeric layout. That encoder bounds individual stage gain, keeps the vendor's first-stage attenuation margin, and compensates in the global scale. All four sample-rate banks are checked after encoding. The original peak-only example is still byte-identical to the listening-tested build.

Without the DLL (on macOS and Linux, or on Windows without the Audeze app), peak/pass-only sets use the built-in encoder too. Its peak stages copy the vendor's layout: each numerator is scaled so b0 is unity, and the last numerator coefficient is rounded so the stage's DC gain stays exact on the 24-bit grid. Without that, low narrow peaks at 88.2 and 96 kHz lose up to 0.4 dB. `tests/encoder.rs` compiles 400 random peak sets both ways: the built-in encoder builds every set the DLL builds, with responses within 0.21 dB of the DLL's, and builds 91 sets the DLL rejects for precision. A −12 dB peak it built was clearly audible on a Mac, but it hasn't been compared with the DLL by ear. To do that on Windows, start the app with `MAXWELL_PEQ_ENCODER=built-in`, which uses the built-in encoder even when the DLL is installed. The editor says which encoder is active.

### Device access

Native HID operations run in a child process (`MaxwellPEQ.exe --worker device`, JSON on stdin and stdout) with a 15-second deadline, because a native driver call can stall; a stalled worker is killed, and the worker reports how far a write got so the app knows whether the headset may have changed. Dongle requests use the headset route flag `0x80`. Persistent writes are restricted to the four custom coefficient keys `E42C` through `E42F`; arbitrary storage writes, erase commands and firmware operations are excluded.

### Filter JSON

Each row accepts `type`: `peak`, `low_shelf`, `high_shelf`, `low_pass` or `high_pass`. A missing type or the legacy `PK` means peak. Shelves may omit Q; pass filters may omit gain and Q. See [examples/shelf-and-pass.json](examples/shelf-and-pass.json). These files keep filter types and are stored on the computer, not in the headset.

## Development

The app is Rust with [Tauri 2](https://tauri.app). The editor in `web/` is plain HTML, CSS and JavaScript with no build step; the Rust binary embeds it, so rebuild after changing it. The workspace has two crates: `crates/maxwell-core` (filter compiler, APO conversion, profiles, HID protocol and preset links; no UI) and `src-tauri` (the window, plus the `--serve` and `--worker device` entry points).

### Build

With [Rust](https://rustup.rs) 1.89 or newer and the Tauri CLI (`cargo install tauri-cli --version "^2"`):

```powershell
cd src-tauri
cargo tauri build --no-bundle   # target\release\MaxwellPEQ.exe, a single file
cargo tauri build               # also an NSIS installer under target\release\bundle
cargo run -p maxwell-peq -- --serve --offline   # debug build, editor in a browser
```

On Linux, install the WebKitGTK and udev headers first (Debian/Ubuntu: `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev libudev-dev`); `cargo tauri build` then builds a `.deb`, `.rpm` and `.AppImage`. On macOS the same commands build `Maxwell PEQ.app` and a `.dmg`; `--target universal-apple-darwin` builds both architectures at once (add the targets with `rustup target add aarch64-apple-darwin x86_64-apple-darwin`).

### Validate

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets
cargo clippy -p maxwell-core --features hidapi --all-targets   # the macOS/Linux HID backend, on Windows
node --check web/app.js
node --check web/graph.js
python -m unittest discover -s tests   # the Python reference
```

The Rust tests include golden parity checks: compiled coefficient blobs, curves, preamps and validation errors must equal the Python output exactly. They also cover the vendor math against reference responses, Q width, coefficient packing, packet allowlists, fragmented HID messages, malformed replies, storage capacity and acknowledgement parsing, exact readback, preset links across restarts and unverified saves, balance and control limits, Equalizer APO import and export, the profile library, and the Audeze app guard. Tests never send hardware writes. Vendor tests need the inspected DLL; the rest run without it.

`.github/workflows/ci.yml` runs formatting, Clippy and the tests on Windows, macOS and Linux for every pull request and every push to `main`.

### Releases

`.github/workflows/release.yml` runs when a `v*` tag is pushed; the tag must match the workspace version in `Cargo.toml`. It runs the tests, builds the Windows installer, the standalone exe, the universal macOS `.dmg` and the Linux `.deb`, `.rpm` and `.AppImage`, and uploads them to a **draft** release, which you publish by hand after checking it:

```powershell
# bump version in Cargo.toml, commit, then:
git tag v0.2.0
git push origin v0.2.0
```

### Python reference implementation

The original Python version (`server.py`, `peq.py`, `device_worker.py` and friends) stays in the repository as the reference the Rust port is checked against. `python tools/make_golden.py` regenerates the fixtures in `crates/maxwell-core/tests/golden/`. `python server.py` still serves the editor at `http://127.0.0.1:8769` with no pip packages, and `./start.ps1` starts it in the background. It uses the older Apply, backup and Restore flow, so saving to presets and switching them only work in the Rust app. Source runs keep profiles and backups in the repository folder unless `MAXWELL_PEQ_DATA` is set. `peq.py` reads a preset JSON object on stdin and writes compiled JSON to stdout; see `examples/one-peak.json`.

## License

Maxwell PEQ is free software under the [GNU General Public License v3.0](LICENSE) or any later version. You can use, study, change and share it. If you distribute a modified version, you must release its source under the same license.

The Audeze app and its `AirohaPeqLibrary.dll` are Audeze's and are not covered by this license; Maxwell PEQ never includes them. Maxwell PEQ is not made or endorsed by Audeze.
