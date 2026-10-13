# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

A Tauri 2 desktop app: the editor is a local web page in the system webview (WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux), backed by a Rust core. It ships for Windows (installer and a standalone exe), macOS 11+ (universal `.dmg`) and Linux (`.deb`, `.rpm`, `.AppImage`). `--serve` shows the same editor in a normal browser on loopback. The vendor coefficient DLL exists only on Windows; every platform has the built-in encoder.

## Users

The owner of an Audeze Maxwell, tuning the headset for themselves at a desk while listening. All hardware testing has been on the original Maxwell PlayStation. The Xbox edition is handled as the same headset and is untested. Maxwell 2 support comes from static analysis of Audeze HQ and is untested; Maxwell 2 ANC is refused. They already use Equalizer APO / AutoEQ-style parametric presets and want to move them onto the headset and switch between several tunings quickly.

## Product Purpose

Edit up to ten parametric filters, preview the calculated response, and save profiles to the headset's four custom presets with byte-exact readback. The editor also switches the active preset, sets left/right channel balance, and covers the everyday Audeze HQ controls (chat mix, sidetone, noise suppression, auto shutdown, voice prompts). Success is a tuning switch that takes one click and an imported Equalizer APO preset that sounds the same on the headset.

## Positioning

Audeze HQ only offers a ten-band graphic EQ. This tool writes real parametric coefficients (peaks, shelves, pass filters) to the headset, verified against the RBJ reference before anything is sent.

## Operating Context

- Audeze HQ must be fully closed while the app talks to the headset (exclusive HID access). The editor offers to close it.
- Connections: USB-C cable or the headset's wireless dongle, chosen automatically or forced from the device bar.
- Profiles are local files. Each of the four custom presets holds one saved EQ, and the editor remembers which profile was saved to which preset.
- Equalizer APO text (`Preamp:` / `Filter n: ON PK Fc … Hz Gain … dB Q …`) is the common exchange format, including AutoEQ output.

## Capabilities and Constraints

- 1–10 filters, 20–20,000 Hz, gain −12 to +12 dB, Q 0.1–20.
- Shelves have a fixed slope (S = 1, Q ≈ 0.707). Pass filters are 12 dB/octave with adjustable Q and no gain.
- Peaks are confirmed by listening on the headset; shelf and pass filters are experimental. A rough midband measurement matched them, but bass and treble are unmeasured and the response plot is calculated, not measured.
- Auto preamp subtracts the peak of the combined curve; manual preamp must give at least that much headroom.
- A rate that can't hold the filters precisely gets the nearest values it can (the other encoder, a slightly higher frequency or lower Q), and the editor says so.
- Editing never touches the headset. Saving to a preset is the only EQ write; switching presets sends only the switch, and the headset loads that preset's stored EQ.
- A save that starts but can't be verified drops the preset's link to its profile rather than claiming it worked.
- Balance writes are capped at 150 per side, skip the firmware-reserved values, and are read back before they count. Headset controls are read back too.
- Device access is on by default; `--offline` disables it.

## Evidence on Hand

PROOF.md records the hardware evidence: byte-exact readback, persistence across power cycles, listening confirmation, preset switching, and a rough midband mic measurement. Balance was heard and read back on hardware; the Headset… controls were decoded from Audeze HQ and are untested. Never present the plot as a measurement.

## Product Principles

1. Never imply the headset changed unless it acknowledged the change.
2. Editing is free; writing to the headset is a deliberate, visible step that is read back.
3. One click to switch tunings.
4. Say what is experimental or unverified, plainly and once.
