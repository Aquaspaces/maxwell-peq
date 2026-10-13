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

- A 236px sidebar holds profiles (Ctrl+1–9 hints on hover) and APO import; the main column holds the header, the Audeze banner, the graph, the filter table and a sticky device bar.
- Graph height is `clamp(230px, 40vh, 440px)`. Range is ±15 dB, 20 Hz–20 kHz on a log axis.
- The device bar always shows one status line with a coloured dot, then the controls in order of consequence: connection, Apply live, Connect, Apply, Save to headset, Restore.

## Components

- **Graph handles**: numbered circles; the selected one is filled. Disabled filters are faint, pass filters sit on the 0 dB line. The selected filter's own curve is a thin dashed line under the total.
- **Curve**: solid once compiled; dashed while the estimate is waiting on compilation; red dashed when compilation failed.
- **Buttons**: primary (blue fill), default (raised grey), ghost (text only), danger, and `warn-solid` for the banner's Yes.
- **Toasts**: bottom right, a coloured dot for the kind, no side stripe.
- **Menus and dialogs** float with a 1px border and a soft shadow; they are the only shadowed layers.

## Motion

Short fades and 4–6px slides for toasts and menus (≤160ms). `prefers-reduced-motion` removes them.
