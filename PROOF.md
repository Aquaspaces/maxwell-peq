# Evidence record — 2026-10-12

Initial investigation target: original Audeze Maxwell PlayStation, direct USB interface VID `3329`, PID `4B1A`, usage page `FF13`. A `4B19` dongle was also present and was not used. The active built-in preset returned raw index `4`. That initial investigation only sent reads. Later live and persistent tests are recorded below.

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

App live function: `0x140024C10`. Saved-coefficient write function: `0x140024EF0` (command `0A0D`); implemented later with the restricted save/readback path described below. Protocol references are from local app disassembly and the supplied [balance/firmware tool source](https://github.com/kats1123/maxwell-balance-and-firmware-tool) at commit `16280b47fef610284beab5d40d46868d7b4c3253`.

## What remains unverified

Validation completed: 23 unit/integration tests passed with the vendor DLL present; JavaScript syntax check passed. Browser checks at 1280 px and 390 px verified rendering without page overflow, four compiled curves, stale-preview invalidation after Q edits, and a successful read of active built-in preset 4 with Apply disabled. Recovery tests use mocked workers, including pre-send preset changes, timeouts, restoration without recompilation and offline-mode gating.

Exact measured response, behavior across reconnects/preset switches, and other firmware revisions remain unverified. The initial format checks have since been supplemented by the user listening test below. Peak filtering has listening confirmation on direct USB original PS hardware. Shelf/pass support has the numerical validation described below; Xbox, Maxwell 2 and Bluetooth remain untested. Dongle testing is recorded below.

## HID contention fix — 2026-10-12

A user Apply attempt failed with `RACE stream lost its packet boundary`. The server remained `unchanged`, meaning its worker reported failure before starting the live write. A read-only trace reproduced intermittent failure while `Audeze.exe` was running: parts of the saved-coefficient reply were missing, followed by HQ polling responses for unrelated commands. This supports competing readers consuming a shared device stream, not an invalid EQ coefficient blob.

After the user fully quit HQ, five consecutive full snapshots passed. The control handle now opens with sharing mode zero for the complete transaction; a sharing violation explicitly asks the user to quit HQ rather than permitting interleaved access. This follows [Windows CreateFile sharing semantics](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilea). Enumeration still uses metadata-only probes. No persistent write or live EQ was sent during this investigation.

Verification: all 25 tests passed. On the actual headset, a second read/write handle was rejected with Windows error 32 while the exclusive handle was open, and reopening after closing the first handle succeeded. The existing server loads the updated transport in each worker, so its in-memory backup was retained without a restart.

## User listening confirmation — 2026-10-12

After the contention fix, Aqua reported that temporary EQ audibly worked on the original Maxwell PlayStation. Adjusting Q produced a clearly audible change. With a 1,000 Hz tone generator and a very narrow -12 dB peaking filter centered at 1,000 Hz, the tone was clearly quieter.

This is user-reported end-to-end listening evidence for live peaking EQ and adjustable Q. It is not a calibrated measurement of exactly -12 dB or of the implemented bandwidth. A tone at the center demonstrates attenuation there; the reported change when varying Q provides separate evidence of bandwidth control. No custom firmware was required for these tests.

## Shelf and pass filter support — 2026-10-12

`set_xpf_point` at DLL VA `0x180001200` has ABI `(int context, int band, double frequency, double Q, int type)`. The generator's branches implement type 1 low-pass and type 2 high-pass. Offline calls at 1 kHz and Q sqrt(0.5) matched RBJ responses across all four rates within 0.00214 dB on the comparison grid. The application now calls this native function for pass filters; peak/pass-only sets retain the vendor generator.

The numeric format has **24 significant bits**, shifted left eight bits in the 32-bit wire value. Stage fractional bits are 23/22/23/22/23; global scale uses 18. DLL code at `0x18000347C` through `0x180003604` corroborates the global scale's 2^18 conversion and floor(x+0.5) rounding. The prior 31/30/26 descriptions specify the wire decode, not 32-bit generation precision.

Shelf-containing sets use `extended_filters.py`, with equations from the [W3C Audio EQ Cookbook](https://www.w3.org/TR/audio-eq-cookbook/). Shelves have fixed S=1. Each stage numerator is normalized by an analytical response bound and representable coefficient range; the first stage retains the vendor's 1.19 attenuation margin. The final scale compensates that normalization and applies preamp. Stage ordering/normalization are not byte-identical to the vendor's implementation. All generated banks are decoded and checked for stability, numerical accuracy and headroom before use.

Pass-filter resonance is included in automatic headroom analytically: for Q > sqrt(0.5), peak magnitude is Q/sqrt(1-1/(4Q^2)). Comparisons include the corresponding digital resonance frequency, DC and Nyquist. Error comparisons for experimental sets floor both responses at -90 dB; this avoids meaningless relative-dB errors at deep stopband nulls while still rejecting leakage above that floor. The 0.15 dB threshold remains: for example a 30 Hz, -6 dB low shelf is rejected for approximately 0.237 dB error, whereas a 100 Hz, +6 dB shelf passes at approximately 0.0363 dB.

Verification: 36 tests passed locally with the inspected DLL, including all new types, mixed cascades, shelf endpoint/midpoint properties, pass resonance, disabled filters, validation and a frozen SHA-256 check that the original peak preset remains byte-identical. Browser compilation returned four rate curves for all new filter types with appropriate gain/Q controls disabled. New filter types remain marked experimental; no new filter was sent to the headset during development.


## Dongle and persistent coefficient storage ? 2026-10-12

The original PS dongle uses VID `3329`, PID `4B19`, usage `FF13`, 62-byte HID reports and route byte `80`. An actual full snapshot through that interface returned current index 10 (Custom 4), its UI records and its 844-byte coefficient blob. The stored bytes matched the earlier USB backup exactly.

Static evidence comes from `Audeze.exe` SHA-256 `88299c36990f44de4ce17a5b9c65b5521fd68ee649fdb83e80ccf530ad604ca5` and the inspected stock PS v74 firmware. Local report: `X:/audeze-re/analysis/persistence/report.md`. The connected headset's firmware version remains unidentified; the stock firmware trace establishes implementation evidence, not its runtime version.

- App function `0x140024EF0` sends `0A0D`, a little-endian key `E42C + slot`, then the 844-byte coefficient blob. The RACE packet totals 852 bytes. Acknowledgement contains status and the same key.
- `0A03` with payload `4C03` is a capacity query for 844 bytes, not garbage collection. Firmware handler `0x081768E0` calls `0x081514F8` to read locked capacity counters; reply 1 means enough space and 0 means insufficient. The implementation never sends a reclaim/erase operation.
- Firmware `0A0D` handler `0x08176838` writes the key and compares readback. The editor independently reads the same key and requires every byte to match before reporting success or sending the live apply.
- Static reload code follows E401 phase 0 to E402. Presets 7?10 map phase 0 to E42C?E42F. Functions `0x081A5982`, `0x081A59FC`, `0x081A5A1E`, `0x081A5A2A` and `0x081A5A9C` select and load these coefficients through `0x081A4C68`. This path does not regenerate coefficients from the EF00?EF03 slider records. It supports coefficient-only persistence; runtime reboot validation is a separate check.

The API adds `/api/save` and `/api/restore_saved`. An immutable original backup is separate from the expected current stored state. Before each write, the server atomically journals the pending operation, original backup and desired bytes. Restart recovers that session. Save/Apply are disabled after uncertain writes; explicit saved restoration can repair mismatched stored coefficients while requiring the same interface, slot and UI records. Restore does not depend on the math DLL.

Hardware save test: with the user's authorization, the editor saved two peaks (1,370 Hz, -12 dB, Q 0.35; 6,200 Hz, -12 dB, Q 0.7; automatic preamp 0 dB) through the dongle to Custom 4. Save ACK was `055b05000d0a002fe4`. All 844 saved bytes matched compiled SHA-256 `f53a4e499baec5c1c403e829137618b70eba3d76ca6225f486a5878947f17833`. The subsequent temporary-apply ACK was `055b0300030e00`. Original coefficients remain in the ignored backups directory, including the earlier USB backup and a fresh matching dongle backup. Raw results: `artifacts/dongle-first-read.json`, `artifacts/dongle-first-save.json`, `artifacts/obvious-persistence-compiled.json`.

Before saving, the old USB session's restoration was stopped before a write: one attempt encountered a stream-boundary error, and a fresh snapshot then detected a changed USB interface path. A new dongle snapshot matched the original stored data before creating the new backup. No failed persistent write occurred in this test.

Validation: 57 automated tests pass, including packet/key restrictions, capacity reply semantics, failed readback preventing live application, verified-save/live-failure recovery, explicit repair of a malformed stored blob, and recovery after server restart or worker timeout. JavaScript syntax passes. An actual server restart recovered the saved state, dongle selection, original backup and last filter JSON. Browser checks confirmed the saved-state message, enabled restoration, disabled fresh backup and no horizontal overflow at 1280 px. Acoustic response remains unmeasured. Aqua unplugged the headset USB cable, left the dongle connected, powered the headset off/on, and reported that the obvious EQ remained in wireless playback. A subsequent read-only snapshot confirmed Custom 4 still contained all 844 expected bytes with the same SHA-256. USB enumeration found no direct headset interface. No EQ was reapplied after reboot. This establishes stored-byte retention and user-reported audible wireless persistence on this headset; it is not an acoustic response measurement. Raw readback: `artifacts/dongle-post-reboot-read.json`.


The first post-reboot snapshot hit a packet-boundary error before any EQ write. A traced fresh read succeeded and showed leftover coefficient fragments and restart notifications. The transport now requires six consecutive empty polls (at least 50 ms) before sending a command, instead of trusting the first empty poll. Input that never settles aborts after 100 polls. Tests cover delayed fragments restarting that interval, malformed input, and the bounded abort. Three consecutive wireless snapshots with the updated drain matched the saved profile. This improves handling of delayed input; it does not claim to eliminate every possible transport failure.
