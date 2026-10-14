## What's new

<!-- Replace this with the changes before publishing. Maxwell PEQ shows this section as the release's patch notes. -->

- **Tray icon.** The icon shows battery, mic mute and the active EQ. Left-click opens a small status panel; right-click switches EQ 1 to 4 and opens Settings and Keyboard shortcuts.
- **Shortcut keys.** Switch EQ from any app without bringing up a window. Ctrl+Alt+1 to 4 pick an EQ and Ctrl+Alt+E steps to the next one. You can rebind them in the editor.
- **Scripting.** `MaxwellPEQ.exe --preset <1-4|next|previous>` switches EQ and `--status` prints the headset state as JSON. Both go through the app if it's running.
- **Settings.** Start when you sign in, show or hide the tray icon, keep running or minimize to the tray when the window closes, and check for updates. When a new version is out, the tray menu offers the download and Settings shows these notes.
- **Shelf Q.** Low and high shelves take a Q from 0.1 to 20 (default 0.7071). Imported `LSC`/`HSC` filters with a slope in dB per octave are converted to Q. Shelf Q above 0.7071 hasn't been checked by ear on a headset yet.
- **Sent to the headset.** A sidebar panel lists the raw reports the app sends to the headset as it sends them, with an option to show the headset's replies. Copy and Clear are next to it.

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
