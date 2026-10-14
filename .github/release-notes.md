## What's new

<!-- Replace this with the changes before publishing. Maxwell PEQ shows this section as the release's patch notes. -->

## Downloads

**Windows 10/11**
- `Maxwell.PEQ_*_x64-setup.exe` installs the app with a Start menu entry.
- `MaxwellPEQ*.exe` is the same app as a single file. Run it from anywhere; nothing is installed.

If the Audeze app is installed, Maxwell PEQ builds filters with its PEQ library. Otherwise it uses its own encoder, which works on a headset but hasn't been compared with the Audeze library by ear. Close the Audeze app while you use Maxwell PEQ.

**macOS 11 or later (Apple silicon and Intel)**
- `Maxwell.PEQ_*_universal.dmg`: open it and drag Maxwell PEQ to Applications.

The Mac build isn't signed by Apple. The first time you open it, macOS blocks it: open System Settings → Privacy & Security and choose **Open Anyway**.

The Mac build has every feature. It always uses Maxwell PEQ's own filter encoder, because the Audeze library only exists on Windows. EQ, restore and balance have been tested on a Mac over USB-C, and saving to the headset over the dongle.

**Linux (x64)**
- `Maxwell.PEQ_*_amd64.deb` for Debian and Ubuntu, `Maxwell.PEQ-*.x86_64.rpm` for Fedora and openSUSE. Both install a udev rule so the app can open the headset without root.
- `Maxwell.PEQ_*_amd64.AppImage` runs without installing, but you need to install the udev rule yourself (see the README's Linux section).

The Linux build uses Maxwell PEQ's own filter encoder, like the Mac build. It has read the headset, read balance and applied live EQ over the dongle; restore and saving to the headset haven't been tested on Linux yet.

Tested on the original Audeze Maxwell (PlayStation), over USB-C or the dongle. Maxwell 2 is supported but untested. Maxwell 2 ANC is not supported.
