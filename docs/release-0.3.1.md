# ShopPhoto 0.3.1 — GUI pass

0.3.1 implements the 18 proposals in [the GUI review](gui-improvements.html).
The develop pipeline, sidecar format and AI models are unchanged; history
entries written by 0.3.1 carry descriptive labels, which older versions show
as plain text.

## Viewport

- **Before/after for every edit.** `\` still toggles the whole frame (now
  labelled on the image). The viewport toolbar adds **Split** (draggable
  divider) and **Side by side**; `Y` cycles them. "Before" is the original, or
  any snapshot or history step via *Compare with current* (right-click a step,
  or the `vs` button on a snapshot). The denoise before/after in Detail uses
  the same renderer.
- **Viewport toolbar**: Fit · Fill · 50 · 100 · 200 %, shadow/highlight
  clipping toggles, before/after modes and a *Proof* chip when soft proofing.
  A **navigator** appears while zoomed; drag it to pan.
- **Tool banner**: every canvas tool (crop, pickers, heal/clone, brushes,
  guided upright, AI subject pick, white-balance picker) shows one readable
  banner with *Cancel* / *Done* (Esc / Return).
- **Crop**: corner and edge handles, thirds grid while dragging, locked aspect
  ratios (Free, Original, 1:1, 4:5, 3:2, 16:9, *Swap* or `X`), a live
  pixel/megapixel readout, Return applies and Esc restores the previous crop.

## Inspector

- **Sliders** fill from their neutral point, show units (`+0.60 EV`,
  `5500 K`, `−14`), turn accent when changed, reveal a reset button on hover,
  scrub from the label (Shift = fine), accept typed values and arrow keys.
- **Temperature** uses a logarithmic scale with a blue→amber track (Tint
  green→magenta). **Pick neutral…** solves Temp and Tint from the RAW so the
  clicked spot renders grey.
- **Cards** show how many controls differ from default, reset one group from
  the header, and can be **bypassed** (eye) to judge a group without an undo
  step. *Solo* (or Option-click) keeps one card open.
- **Pinned header**: the full-height histogram, an EXIF line, tool search
  (⌘F) and a section jump bar stay visible while the cards scroll. Specialist
  controls live under **Advanced** in Basic, Detail, Lens, Geometry and HDR.
- **Histogram**: hovering tints and names the region its drag edits, with the
  live value; corner triangles light up when the image clips and toggle the
  clipping overlays.
- **Color mixer** shows all eight bands on colored tracks (Hue / Sat / Lum),
  and color grading uses real wheels (drag hue and amount, Shift keeps hue,
  double-click resets).
- **History** names every step ("Exposure +0.60 EV", "Preset: Film Warm",
  "Basic · 3 changes"), shows relative times, previews on hover and jumps on
  click; later steps stay as redo until the next edit.
- **Presets** are thumbnails of the open photo rendered through each preset,
  preview on hover, and have an **Amount** slider; edits after applying show a
  *modified · Re-apply* chip.

## Library and filmstrip

- Click selects in the grid; Shift extends, ⌘ toggles, double-click or Return
  opens. Arrow keys move focus. Ratings, flags and labels apply to the whole
  selection (and are written to each photo's sidecar/XMP). Stars are clickable
  on hover; picks and rejects have icons and rejects are dimmed.
- Filters: search, a star threshold, flag and color-label toggles and *More
  filters*; active filters appear as removable chips with "Showing N of M".
  File operations moved to a selection bar; **Rename…** previews new names.
- The filmstrip keeps each photo's aspect ratio, distinguishes the open photo
  (ring, raised) from other selected photos (wash), shows rating/flag/edited/
  label badges, a position counter with previous/next, and a loading shimmer.

## Feedback and keyboard

- A **progress ring** in the top bar shows background work (Auto Finish,
  export, import, model download, AI work, folder scans); click for details
  and Cancel. The SDR/HDR badge is a fixed-height pill.
- **Toasts** count down visibly, pause on hover, can be dismissed, offer
  **Undo** for file moves/renames and settings sync, and errors stay until
  dismissed with *Copy details*. Trash has no Undo: files go to the operating
  system trash.
- The **shortcut sheet** is complete and searchable (see
  [shortcuts](shortcuts.md)). Tab only hides panels when no control has focus,
  and single-letter shortcuts no longer fire with ⌘ (⌘E no longer exports).

## QA hook

`SP_TEST_UI=edit,library,select,split,sbs,crop,zoom,panels,color,sheet,toast,bypass`
puts the UI into those states before an `SP_SCREENSHOT` capture. Sidecars are
not saved while it is set, so the throwaway edits never reach real photos.
