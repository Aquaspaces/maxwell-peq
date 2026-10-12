# Evidence record — 2026-10-12

Target: original Audeze Maxwell PlayStation, direct USB interface VID `3329`, PID `4B1A`, usage page `FF13`. A `4B19` dongle was also present and was not used. The active built-in preset returned raw index `4`. This work only sent reads to the actual headset; no live EQ, saved preset or firmware writes were sent.

## Coefficient generator

Inspected local `X:/audeze-re/extract/audeze-app/AirohaPeqLibrary.dll`, SHA-256 `322ac1149eeba9ed55f2466ba706edc92dcdb333cf574a6466abf88e4b3d50be`. The compiler refuses other hashes. The library is vendor code and is not included in this repository.

Disassembly of `set_peq_point` at `0x1800011C0` stores frequency, gain and width. In `generate_cofe`, `0x180002100` divides frequency by width; `0x180002115` doubles that result, and `0x180002119` divides sin(omega) by it. Thus width is linear bandwidth in Hz, **Q = frequency / width**, with alpha = sin(omega)/(2Q). Runtime comparisons confirm peaking-biquad behavior. Local assembly: `X:/audeze-re/analysis/peq-generate-full.asm`.

The independent reference follows the [W3C Audio EQ Cookbook](https://www.w3.org/TR/audio-eq-cookbook/) peaking filter equations. The initial 1 kHz / -3 dB / Q 1 filter had maximum sampled quantization error 0.0028624 dB across four banks. Changing Q changes width while preserving center gain. These are numerical checks, not acoustic measurements.

App packing function `0x1400152B0` creates banks `(1,44100)`, `(2,48000)`, `(5,88200)`, `(6,96000)`. Each has 103 little-endian 16-bit words: band count 10, ten stages of five signed coefficients stored high-word first, and one global scale. Fractional bits are `[31,30,31,30,31]` for `[b0,b1,b2,a1,a2]`; scale is Q26. Four-byte bank count plus four `(id:uint16,count:uint16,words)` records totals **844 bytes**.

## Exact reproduction of a saved headset preset

Read UI records from NVDM key `EF00` and saved coefficients from `E42C` (custom slot 0). Its ten frequencies were 32, 64, 125, 250, 500, 1000, 2000, 4000, 8000 and 16000 Hz; gains 3, 5, 7, 4, -4, -2, 3, 0, 5 and 0 dB; widths implied Q 2 throughout.

Recreating these with the vendor generator and the stock preset's zero global attenuation produced **844/844 matching bytes**, zero differences. Both blobs hash to `a6be633b31b7a5a04f4061ef2a93ca2609b1192bf47a09fc2e8917a7988a4194`. The normal UI compiler enforces additional headroom and precision requirements, so this comparison used the low-level vendor bank generator to reproduce stock settings exactly.

Local raw evidence is in ignored `artifacts/custom-slot0-read.json`, `artifacts/custom-slot0-coefficients.json` and `artifacts/stock-reproduction.json`. Device paths and raw snapshots stay local. Exact saved-byte recovery does not need the math DLL.

## Transport

RACE request: `05 5A`, little-endian length including the two-byte command, command, payload. Output HID report: `06`, chunk length, route byte, up to 59 packet bytes, padding to 62. Direct USB route is `00`. Responses use report ID `07`, with length at byte 1 and metadata at byte 2; assemble RACE packets by their declared length.

- Read current index: command `0901`, payload `0000`.
- Read saved records: command `0A0C`, key `EF00+slot` for UI or `E42C+slot` for coefficients, requested size 1000. Reply includes status, key, stored length and data.
- Custom raw indices 7-10 map to storage slots 0-3.
- Temporary live coefficients: command `0E03`, payload `00` followed by the 844-byte blob. Require exact success acknowledgement `05 5B 03 00 03 0E 00`.

App live function: `0x140024C10`. Saved-coefficient write function: `0x140024EF0` (command `0A0D`); this write is deliberately absent from the proof of concept. Protocol references are from local app disassembly and the supplied [balance/firmware tool source](https://github.com/kats1123/maxwell-balance-and-firmware-tool) at commit `16280b47fef610284beab5d40d46868d7b4c3253`.

## What remains unverified

Validation completed: 23 unit/integration tests passed with the vendor DLL present; JavaScript syntax check passed. Browser checks at 1280 px and 390 px verified rendering without page overflow, four compiled curves, stale-preview invalidation after Q edits, and a successful read of active built-in preset 4 with Apply disabled. Recovery tests use mocked workers, including pre-send preset changes, timeouts, restoration without recompilation and offline-mode gating.

Exact measured response, behavior across reconnects/preset switches, and other firmware revisions remain unverified. The initial format checks have since been supplemented by the user listening test below. This build supports only peaks on direct USB original PS hardware; it makes no claim about shelves, pass filters, Xbox, Maxwell 2, Bluetooth or dongle transport.

## HID contention fix — 2026-10-12

A user Apply attempt failed with `RACE stream lost its packet boundary`. The server remained `unchanged`, meaning its worker reported failure before starting the live write. A read-only trace reproduced intermittent failure while `Audeze.exe` was running: parts of the saved-coefficient reply were missing, followed by HQ polling responses for unrelated commands. This supports competing readers consuming a shared device stream, not an invalid EQ coefficient blob.

After the user fully quit HQ, five consecutive full snapshots passed. The control handle now opens with sharing mode zero for the complete transaction; a sharing violation explicitly asks the user to quit HQ rather than permitting interleaved access. This follows [Windows CreateFile sharing semantics](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilea). Enumeration still uses metadata-only probes. No persistent write or live EQ was sent during this investigation.

Verification: all 25 tests passed. On the actual headset, a second read/write handle was rejected with Windows error 32 while the exclusive handle was open, and reopening after closing the first handle succeeded. The existing server loads the updated transport in each worker, so its in-memory backup was retained without a restart.

## User listening confirmation — 2026-10-12

After the contention fix, Aqua reported that temporary EQ audibly worked on the original Maxwell PlayStation. Adjusting Q produced a clearly audible change. With a 1,000 Hz tone generator and a very narrow -12 dB peaking filter centered at 1,000 Hz, the tone was clearly quieter.

This is user-reported end-to-end listening evidence for live peaking EQ and adjustable Q. It is not a calibrated measurement of exactly -12 dB or of the implemented bandwidth. A tone at the center demonstrates attenuation there; the reported change when varying Q provides separate evidence of bandwidth control. No custom firmware was required for these tests.
