# Design

Maxwell PEQ is a work tool: a dark, neutral EQ editor in the family of desktop audio software. The graph is the centre of attention; everything else stays quiet so the curve and the device state read first.

## Colour

One accent, neutral greys, and status colours used only for status.

| Token | Value | Use |
|---|---|---|
| `--bg` | `#1e1f22` | Main area |
| `--sidebar` | `#18191b` | Profile sidebar |
| `--surface` / `--surface-2` | `#26272b` / `#2b2c30` | Graph panel, dialogs, selected rows |
| `--raised` | `#313238` | Hovered controls |
| `--line` / `--line-strong` | `#34363b` / `#44464d` | Dividers / control borders |
| `--text` / `--muted` / `--faint` | `#e3e4e6` / `#a1a3a8` / `#8e9096` | Text levels; `--faint` stays at 4.5:1 or better on every surface |
| `--accent` | `#3b82f6` | Curve, handles, selection, focus |
| `--button` | `#2563eb` | Primary button fill |
| `--warn`, `--warn-bg` | `#f5c66b`, `#3a2e12` | Audeze banner, paused or uncertain device state |
| `--danger` / `--ok` | `#f87171` / `#4ade80` | Errors, delete / connected, saved |

## Type

Segoe UI Variable (system UI fallbacks), 13–14px body, tabular numerals for every value. Section labels are 11px uppercase in `--faint`. The profile name is the only large text.

## Layout

- A `clamp(264px, 24vw, 304px)` sidebar holds the four headset presets, the profiles not on the headset (one per row, ellipsised), and APO import pinned to the bottom; the main column holds the header, the Audeze banner, the graph, the filter table and a sticky device bar.
- Graph height is `clamp(230px, 40vh, 440px)`. Range is ±15 dB, 20 Hz–20 kHz on a log axis.
- The device bar always shows one status line with a coloured dot and up to two lines of detail, then the controls: connection, Read headset, Headset…, Balance…. Offline shows the status only. The bar never wraps.

## Components

- **Graph handles**: numbered circles; the selected one is filled. Disabled filters are faint, pass filters sit on the 0 dB line, and a filter with an out-of-range frequency sits at the nearest axis edge with a red dashed ring. The wheel changes Q only over a handle or a focused one. The selected filter's own curve is a thin dashed line under the total.
- **Curve**: solid once compiled; dashed while the estimate is waiting on compilation; red dashed when compilation failed.
- **Buttons**: primary (blue fill), default (raised grey), ghost (text only), danger, and `warn-solid` for the banner's Yes.
- **Filter table**: values are right-aligned against their unit; gain shows at least one decimal and Q at least two. The selected row is a surface tint, with no side stripe.
- **Switches**: 34×20 track, blue fill when on, for every on/off headset setting.
- **Toasts**: bottom right, a coloured dot for the kind, no side stripe.
- **Menus and dialogs** float with a 1px border and a soft shadow; they are the only shadowed layers.

## Motion

Short fades and 4–6px slides for toasts and menus (≤160ms). `prefers-reduced-motion` removes them.

## Tray panel

The tray panel is the one surface styled after Windows 11 rather than the editor, so it sits beside the system's own quick settings and volume panels.

- Follows the Windows light or dark mode and the Windows accent colour, read from the registry each time it opens. Acrylic background, rounded corners, Segoe UI Variable.
- A 270-degree arc gauge for the battery with the percentage in the centre; it turns red at 15% or below.
- Bottom left: a round mic indicator, filled with the accent when the mic is muted. Bottom right: the active EQ as an accent pill that opens the EQ list.
- Opens above the tray with a 12px margin and hides when it loses focus or on Esc.
- The tray icon is the same glance in miniature, drawn at four times its size and averaged down: a 240-degree battery gauge with the number inside, in the taskbar's text colour (red at 15% or below); a mic at bottom left, red with a slash when muted; and a green pill at bottom right with the EQ number. Drawn in `src-tauri/src/glance.rs`; `MAXWELL_PEQ_ICON_PREVIEW=<folder> cargo test -p maxwell-peq glance` writes PNGs of it.

## Settings

- Laid out like a Windows Settings page: each row has a title, a one-line note under it, and the switch on the right. Rows that depend on the tray icon dim while it is off.
- Update status is one line with **Check now**, and a primary **Download x.y.z** button only when there is a newer release.
- Release notes are collapsible cards, newest open, with a **New** tag on releases newer than this build. They show headings, lists, bold, code and link text from the markdown, built as DOM nodes; nothing in them can run or navigate the window.
