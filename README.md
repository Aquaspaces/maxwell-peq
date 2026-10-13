# Maxwell PEQ

Desktop editor for up to ten filters on the Audeze Maxwell, connected by USB-C or its wireless dongle. It has been tested only on the **original Maxwell PlayStation** (see [Headsets](#headsets)). Supports peaks, low/high shelves and low/high-pass filters, with a draggable response graph, named profiles, Equalizer APO import/export, automatic preamp headroom, saving profiles to the headset's four custom presets with exact readback, switching the active preset, left/right channel balance, and the everyday Audeze HQ controls. It runs on Windows, macOS and Linux.

The connected headset's saved custom preset was read and reproduced **byte for byte** across all four sample-rate banks. On 2026-10-12, the user confirmed audible live filtering, changes when adjusting Q, and attenuation of a 1 kHz tone with a narrow -12 dB peak. On 2026-10-13, a rough mic measurement matched the expected peak, shelf and pass responses within about 0.2 dB RMS from 300 Hz to 3 kHz; bass and treble response remain unmeasured. See [PROOF.md](PROOF.md) for the evidence and limits.

## Headsets

| Headset | USB product IDs | Status |
| --- | --- | --- |
| Original Maxwell PlayStation | `4B1A` USB-C, `4B19` dongle | Tested |
| Maxwell 2 | `4B2A` USB-C, `4B28`/`4B29` dongle | Supported but untested |
| Maxwell 2 ANC | `4BA8`–`4BAA` | Not supported; untested and unknown |
| Original Maxwell Xbox | | Untested |

Maxwell 2 support comes from static analysis of Audeze HQ 1.2.48 only. HQ sends Maxwell 2 EQ through the same live and save commands as the original, and that is what this app sends. After a save over USB-C, HQ also reselects the active custom preset so the headset reloads it, muting voice prompts for that second; the app does the same, on Maxwell 2 over USB-C only. Nobody has run this app against a Maxwell 2 yet, so start at low volume. Channel balance and the **Headset…** controls stay limited to the original Maxwell.

HQ reads Maxwell 2 ANC EQ with a different command and data layout. What the ANC stores, and whether any of this applies to it, is unknown, so the app refuses it before sending anything.

## Download

Get the latest build from [Releases](https://github.com/Aquaspaces/maxwell-peq/releases):

- **Windows:** the `-setup.exe` installer, or the bare `MaxwellPEQ.exe`, which runs from any folder without installing.
- **macOS 11+:** the universal `.dmg` (Apple silicon and Intel). It isn't signed by Apple, so the first launch is blocked: open System Settings → Privacy & Security and choose **Open Anyway**.
- **Linux (x64):** the `.deb` (Debian, Ubuntu), the `.rpm` (Fedora, openSUSE) or the `.AppImage`. The `.deb` and `.rpm` install a udev rule that lets you open the headset without root. With the AppImage, install the rule yourself; see [Linux](#linux).

## Start

Run `MaxwellPEQ.exe`. It opens its own window (WebView2, built into Windows 10 and 11) and needs nothing else installed. Options:

- `--offline`: edit and export without ever opening the headset.
- `--serve [--port N]`: serve the editor at `http://127.0.0.1:8769/` for a normal browser instead of opening a window.

When the Audeze app is installed (`C:\Program Files\Audeze\AudezeApp`), the app builds peak and pass filters with its `AirohaPeqLibrary.dll`; the Audeze app itself must not be running. The DLL is never bundled. It is checked against the inspected SHA-256 before use; set `MAXWELL_PEQ_DLL` to use another copy, or place the DLL next to the exe. Without it, the built-in encoder is used instead (see [Coefficient paths](#coefficient-paths)).

Profiles and balance write logs are kept in `%LOCALAPPDATA%\MaxwellPEQ` (set `MAXWELL_PEQ_DATA` to move them). Older versions also kept preset backups and a `session.json` in a `backups` folder; this version ignores them, and you can delete the folder.

### Build

The app is Rust with [Tauri 2](https://tauri.app); the editor in `web/` is plain HTML, CSS and JavaScript with no build step. With [Rust](https://rustup.rs) 1.89 or newer and the Tauri CLI (`cargo install tauri-cli --version "^2"`):

```powershell
cd src-tauri
cargo tauri build --no-bundle   # target\release\MaxwellPEQ.exe, a single file
cargo tauri build               # also an NSIS installer under target\release\bundle
cargo run -p maxwell-peq -- --serve --offline   # debug build, editor in a browser
```

On Linux, install the WebKitGTK and udev headers first (Debian/Ubuntu: `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev libudev-dev`); `cargo tauri build` then builds a `.deb`, `.rpm` and `.AppImage`. On macOS the same commands build `Maxwell PEQ.app` and a `.dmg`; `--target universal-apple-darwin` builds both architectures in one (add both targets with `rustup target add aarch64-apple-darwin x86_64-apple-darwin`).

The workspace has two crates: `crates/maxwell-core` (filter compiler, APO conversion, profiles, HID protocol, session recovery; no UI) and `src-tauri` (the window, plus the `--serve` and `--worker device` entry points).

### Releases

`.github/workflows/release.yml` runs when a `v*` tag is pushed. The tag must match the workspace version in `Cargo.toml`. It runs the tests, then builds the Windows installer, the standalone exe, the universal macOS `.dmg` and the Linux `.deb`, `.rpm` and `.AppImage`, and uploads them to a **draft** release, which you publish by hand after checking it:

```powershell
# bump version in Cargo.toml, commit, then:
git tag v0.2.0
git push origin v0.2.0
```

`.github/workflows/ci.yml` runs formatting, Clippy and the tests on Windows, macOS and Linux for every pull request and every push to `main`.

### Python reference implementation

The original Python version (`server.py`, `peq.py`, `device_worker.py` and friends) stays in the repository as the reference the Rust port is checked against. `python tools/make_golden.py` regenerates the fixtures in `crates/maxwell-core/tests/golden/`, and the Rust tests require exact floating-point equality with them. `python server.py` still serves the same editor at `http://127.0.0.1:8769` with no pip packages; source runs keep profiles and backups in the repository folder (the Python version still uses the older Apply, backup and Restore flow, so the editor's preset saves and switching only work in the Rust app) unless `MAXWELL_PEQ_DATA` is set. `./start.ps1` starts it in the background.

## Audeze App/HQ must be closed

The headset's control interface can only have one reader; the Audeze app and this editor would consume each other's replies. While the Audeze app is running, the editor shows "Audeze App/HQ is open. Custom PEQ will not work if it is open, would you like to close it?" **Yes** closes it, including its tray process. **No** keeps it open and pauses every headset action until it is gone; the device bar keeps a **Close it…** link. The check runs again before every headset operation.

## Profiles

The sidebar holds named profiles. Click one, or press **Ctrl+1** to **Ctrl+9**, to edit it. Editing never touches the headset; nothing is sent until you save a profile to one of its presets.

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

1. Connect the headset by USB-C, or connect its wireless dongle with the headset powered on and paired. The device bar's connection is **Automatic** by default: it uses USB-C when the cable is plugged in and the dongle otherwise. Choose USB-C or the dongle there to force one.
2. Close the Audeze app (the editor offers to).
3. The editor reads the headset when it opens; click **Read again** after changing presets or connections on the headset.
4. Drag a profile from **Not on the headset** onto the box next to **Preset 1** to **Preset 4**, or use **Save to Preset N** in its **⋯** menu. Start at low listening volume.
5. Click a preset's dot or name to make it active. You can also switch under **Headset…** or on the headset itself; read it again afterwards to update the editor.

The headset's custom presets are indexes 7 to 10, which the Audeze app shows as Preset 1 to 4. The active one has a green dot, and the device bar names it. Built-in presets can be read but not written. Firmware updates are not supported; use the Audeze app for those.

## Save to a preset

Saving overwrites that preset's four coefficient banks and reads them back byte for byte. If the preset is the active one, the same EQ also plays right away; otherwise it plays the next time that preset becomes active. Switching presets sends only the switch, the same command the Audeze app uses, and the headset loads the preset's stored EQ itself.

Each preset remembers which profile was saved to it and shows that profile next to it; profiles that are on no preset stay in the list below. If you edit the profile afterwards, the preset says **Edited since saving** with an **Update** button. A link is dropped when the headset reports different EQ in that preset, for example after the Audeze app rewrites it, or when a save starts but can't be verified.

The Audeze app's ten slider records are left unchanged because they cannot describe these custom filters. Its sliders will show old values, and moving them can replace the saved EQ. Reselecting the preset in the Audeze app rewrites it from those sliders, which puts its own EQ back. Keep profiles or APO exports for editable copies of yours.

Choosing another connection in the device bar reads the headset through it, and later writes use it. The status line shows which connection Automatic found.

Hardware testing on 2026-10-12 confirmed a save to Custom 4 through the dongle, exact 844-byte readback after powering the headset off/on with USB unplugged, and user-reported audible EQ retention during wireless playback. See [PROOF.md](PROOF.md). [examples/obvious-persistence.json](examples/obvious-persistence.json) contains the deliberately exaggerated two-cut test profile.

## Channel balance

**Balance…** in the device bar (original Maxwell only) sets the left and right driver levels, for headsets whose channels don't match. The dialog reads both levels through the selected connection when it opens (**Read headset** reads again), and the app keeps the first reading so **Put back** can always return to it. Change a side by one to three steps at a time and listen; higher values are louder. The headset applies each write immediately.

- Values run from 0 to 150. 150 is a hard ceiling: higher values can damage the drivers, so the app never sends them. 136 and 142 are skipped because the firmware reserves them.
- Factory values on current firmware are 141 / 149 on the USB-C profile and 147 / 147 on the wireless profile.
- Balance is saved on the headset, but the headset only loads the saved values at power-on when its source profile is USB-C. On the wireless profile it loads its wireless defaults instead, so the dialog offers to switch the profile. The switch takes effect at the next power-on.
- A factory reset or a fully drained battery returns the factory values.

Balance writes are limited to the two level settings and the source profile, and each value is read back twice before it counts. On 2026-10-12 a write of 147/144 over USB-C from a Mac was heard leaning left, and 147/147 was then written back and read back.

The balance protocol, value limits and persistence behaviour come from kats1123's [maxwell-firmware-research](https://github.com/kats1123/maxwell-firmware-research) (CC BY 4.0) and [maxwell-balance-and-firmware-tool](https://github.com/kats1123/maxwell-balance-and-firmware-tool) (MIT). No code, DLLs or firmware files from those repositories are included.

## Headset controls

**Headset…** in the device bar opens the settings Audeze HQ shows on its Sound and Product Settings pages: game/chat mix, volume limiter, sidetone and its level, noise suppression (off, low, high), auto shutdown (5 minutes to never) and voice prompt volume. It also shows the battery level, whether the mic is muted, and the headset's firmware version (and the dongle's, over the dongle). Each change goes to the headset at once and is read back before it counts. If the read-back doesn't match, the dialog says so and asks you to read again.

The same dialog switches between the ten onboard presets. The headset loads the new preset's own EQ, so switching is refused while EQ sent from this app is playing or saved; restore it first. After a switch the app reads the headset again and, on EQ 1 to EQ 4, backs up the new preset so you can edit it.

These commands were decoded from Audeze HQ 1.2.48 and have not been tested on a headset yet. Factory reset and the audio driver reinstall are left out on purpose.

## macOS and Linux

The macOS build reaches the headset through [hidapi](https://github.com/ruabmbua/hidapi-rs) and reads report sizes from the HID report descriptor (`crates/maxwell-core/src/descriptor.rs`). It does everything the Windows build does. The Audeze DLL only exists on Windows, so the Mac always uses the built-in encoder. The Audeze app check looks for processes named `Audeze`, `Audeze HQ` and `AudezeHQ`. On 2026-10-12, on an M1 Pro Mac over USB-C, it read and backed up the headset, applied built-in-encoder peaks and stacked high- and low-pass filters that were clearly audible, restored the backup, and changed balance and put it back. It then switched to the PlayStation dongle, read the headset again, and saved a flat profile to headset storage, which read back exactly. The app window opens and shows the same state.

Profiles live in `~/Library/Application Support/MaxwellPEQ` on macOS and `$XDG_DATA_HOME/maxwell-peq` (usually `~/.local/share/maxwell-peq`) on Linux.

### Linux

Linux uses the same hidapi backend, through `/dev/hidraw`. Those nodes belong to root, so the app needs the udev rule in [`src-tauri/linux/70-maxwell-peq.rules`](src-tauri/linux/70-maxwell-peq.rules), which gives the logged-in user access to the Maxwell's control interface over USB-C and the dongle. The `.deb` and `.rpm` install it to `/usr/lib/udev/rules.d` and reload udev. For the AppImage or a source build:

```sh
sudo cp src-tauri/linux/70-maxwell-peq.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger --subsystem-match=hidraw
```

Then unplug and replug the headset or dongle. Without the rule, the app says it has no permission to open the headset.

On 2026-10-12 the Linux build was tested in WSL2 (Ubuntu 26.04, kernel 6.18) with the dongle passed through by [usbipd-win](https://github.com/dorssel/usbipd-win). It read Custom 4 back byte for byte (the same SHA-256 as the Windows readback), read the balance, and applied a live EQ, which the headset acknowledged with the same reply it gives on Windows. The first read after attaching failed once with a packet-boundary error and the next three passed. Restore, saving to the headset, USB-C and the desktop window have not been tested on Linux, and nothing has been tested on a native Linux machine.

## Supported values and limits

- 1-10 filters of any supported type; 20-20,000 Hz. Disabled rows are replaced with unity filters.
- Peaks: -12 to +12 dB, Q 0.1-20. This path has user listening confirmation.
- Low/high shelves: -12 to +12 dB, fixed monotonic slope S=1 (Q approximately 0.7071). Frequency sets the shelf midpoint. Shelf slope is not adjustable in this version.
- Low/high-pass: second order (12 dB/octave asymptotic slope), Q 0.1-20. Gain is unused and must be 0. Q approximately 0.7071 gives the Butterworth response; larger Q adds resonance near the cutoff.
- **Shelf and pass filters are still marked experimental:** numerical checks pass, and a rough measurement on the headset matched them from 300 Hz to 3 kHz (see [PROOF.md](PROOF.md)). Their bass and treble response has not been measured. The type selector and compiled result identify them.
- Auto preamp subtracts the peak of the combined response, sampled at every rate around each filter and its pass-filter resonance, rounded up to 0.01 dB. Overlapping boosts and cuts offset each other, so it is usually far less than the sum of the boosts. If quantization still lifts the compiled response past +0.15 dB, Auto preamp drops a little further. Manual preamp must provide at least the curve's peak; maximum attenuation is 60 dB.
- Compilation checks pole stability and compares decoded quantized response against the RBJ reference. It rejects sampled errors over 0.5 dB (narrow sub-bass peaks come close at 88.2 and 96 kHz). For shelf/pass sets, comparisons floor both curves at -90 dB to handle deep stopband nulls and also check DC/Nyquist and pass-filter resonance. When a bank exceeds the precision limit, the app sends the nearest one that doesn't: at that rate it first tries the built-in encoder in place of the DLL, then raises one filter's frequency (up to an octave, lowest filters first) or lowers its Q (down to a quarter), in the smallest steps the editor shows. Other rates keep the exact values, and the editor notes each change. Only sets that no such change fixes are rejected.
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
