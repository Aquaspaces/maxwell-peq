# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

The interface is a local web page shown in a desktop window (pywebview on WebView2). The first packaged target is a single Windows executable. Keep packaging and data-path code portable so macOS and Linux builds stay possible; the HID transport and the vendor coefficient DLL are Windows-only today.

## Users

The owner of an original Audeze Maxwell (PlayStation version), tuning the headset for themselves at a desk while listening. They already use Equalizer APO / AutoEQ-style parametric presets and want to move them onto the headset and switch between several tunings quickly.

## Product Purpose

Edit up to ten parametric filters, preview the calculated response, and send them to the headset: temporarily (live DSP) or saved into the active custom preset slot with byte-exact readback. Success is a profile switch that takes one click and an imported Equalizer APO preset that sounds the same on the headset.

## Positioning

Audeze HQ only offers a ten-band graphic EQ. This tool writes real parametric coefficients (peaks, shelves, pass filters) to the headset, verified against the RBJ reference before anything is sent.

## Operating Context

- Audeze HQ must be fully closed while the app talks to the headset (exclusive HID access).
- Connections: USB-C cable or the PlayStation wireless dongle.
- Profiles are local files; the headset holds one custom EQ per custom slot.
- Equalizer APO text (`Preamp:` / `Filter n: ON PK Fc … Hz Gain … dB Q …`) is the common exchange format, including AutoEQ output.

## Capabilities and Constraints

- 1–10 filters, 20–20,000 Hz, gain −12 to +12 dB, Q 0.1–20.
- Shelves have a fixed slope (S = 1, Q ≈ 0.707). Pass filters are 12 dB/octave with adjustable Q and no gain.
- Peaks are confirmed by listening on the headset; shelf and pass filters are experimental. The response plot is calculated, not measured.
- Auto preamp subtracts the sum of maximum boosts; manual preamp must give at least that much headroom.
- Every device write is journaled; after an uncertain write, Apply/Save are blocked until a restore.
- Switching profiles applies them live (temporarily). Saving to headset flash is a separate, deliberate action.
- Device controls are on by default in the packaged app; `--offline` disables them.

## Evidence on Hand

PROOF.md records the hardware evidence (byte-exact readback, reboot persistence, user listening confirmation). No acoustic measurements exist; never present the plot as one.

## Product Principles

1. Never imply the headset changed unless it acknowledged the change.
2. Make the safe path the easy path: backup before writing, restore always reachable.
3. One click to switch tunings; deliberate steps to write flash.
4. Say what is experimental or unverified, plainly and once.
