# Keyboard shortcuts

The in-app sheet (`?`) is generated from the same table
(`crates/sp-app/src/app/shortcuts.rs`) and is searchable; pressing a key while
it is open highlights that key's row. Single-letter shortcuts ignore ⌘/Ctrl/⌥
chords, and Tab/arrow keys go to a focused control before the app.

| Keys | Action |
|---|---|
| Cmd/Ctrl+O | Open RAW |
| D / G | Develop / Library grid |
| Left / Right | Previous / next photo (grid: move focus) |
| Up / Down | Grid: move focus by a row |
| Shift + arrows | Grid: extend the selection |
| Return | Grid: open the focused photo · Crop: apply |
| Cmd/Ctrl+A | Grid: select all |
| Esc | Clear selection · leave compare · cancel the active tool |
| C | Compare 2–4 selected photos side by side |
| Tab | Hide/show rails (when no control has focus) |
| Z | Fit / 100% |
| `\` | Before / after (whole frame) |
| Y | Cycle split / side-by-side / off |
| J / O | Shadow / highlight clipping overlay |
| Alt while dragging | Temporarily show both clipping thresholds |
| M | AI mask coverage |
| 0–5 | Rating (grid: every selected photo) |
| P / X / U | Pick / reject / unflag (X swaps the ratio while cropping) |
| 6 / 7 / 8 / 9, Shift+9 | Red / yellow / green / blue, purple label |
| Cmd/Ctrl+Z, Shift+Cmd/Ctrl+Z | Undo / redo |
| E | Export |
| Cmd/Ctrl+F | Find a tool in the inspector |
| F12 | Performance overlay |
| ? | Shortcut sheet |

Mouse: drag a slider's label to scrub it (Shift for fine steps), double-click a
label or track to reset, Shift-double-click Exposure for Auto Exposure,
Option-click a card header to open only that card.
