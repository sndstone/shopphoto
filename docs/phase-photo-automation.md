# Photo workflows, automation and client delivery

The subsequent [Lightcraft lessons phase](phase-lightcraft-lessons.md) adds
versioned develop commands, JSONL scripts, a desktop command console and an
MCP stdio session to the renderer workflows described here.

Tools → Photo workflows saves a versioned `.spworkflow` containing selected
develop groups and the current export settings. Applying a saved workflow
creates one develop undo step and loads its delivery settings. Camera profile,
white balance, crop, local masks and HDR can each be included explicitly.
Photo captions, ratings, copyright and other identity metadata are retained
from the target photo, not recorded in the export recipe. Profiles, models and
watermark images referenced by path must remain available.

The same renderer is available without opening a desktop window:

```sh
shopphoto workflow-init delivery.spworkflow
shopphoto render input.jpg --output result.png --exposure 0.3
shopphoto render input.dng --output result.jpg --workflow delivery.spworkflow
shopphoto queue export-queue.json
```

`workflow-init` writes a template that can be edited as JSON. `render` reads the
photo's saved edits, optionally applies a workflow, and supports `--settings`
with an export preset/settings JSON. Without settings, the destination extension
selects the format. A settings/extension mismatch is rejected. These commands
require a compatible GPU. They preserve source originals and sidecars, produce
newline JSON results, and return a nonzero status on errors. A queue continues
past a failed photo, recording every outcome before proceeding.

Queue owners hold an OS file lock across atomic queue saves. Close the desktop
app before running its queue from the CLI; another process cannot claim the same
queue. An interrupted task becomes pending on the next load. The desktop waits
for Resume. The CLI command is itself an explicit request to run pending tasks.
Completed tasks are not repeated. Failed tasks require Retry in the queue UI.

Tools → Proof gallery / slideshow renders selected photos with their edits into
a new portable folder. It contains sRGB JPEGs limited to a 2048-pixel long edge,
an offline browser gallery, a slideshow, selection checkboxes and star ratings.
Current export watermarks are retained. Original paths, camera EXIF and GPS are
not included; filenames and optionally captions are visible. No service upload,
email or other external sharing is performed.

Clients open `index.html`, make choices and use Save picks to download a
`.sppicks` file. Browser storage retains choices where local storage is supported;
the downloaded file is the portable copy. ShopPhoto keeps a private receipt in
its local `shopphoto/proof-receipts` directory. Import returned picks validates
that receipt and every photo ID, then shows a review before applying choices.
Picks and nonzero ratings are applied together through the conflict-checked
sidecar persistence lane. Unselected/unrated entries do not erase existing
annotations. Photos absent from the current catalog are identified and skipped.
Receipts currently match source paths; a moved photo must be relinked to its
receipt path or reissued in a new gallery before importing its choices.

Tools → Workspaces, shortcuts & accessibility stores named panel arrangements,
Library/Develop mode, side-panel visibility and interface scale. Seventeen
editing/navigation actions have saved shortcut overrides with collision checks;
arrow, rating, text-editing and system keys remain reserved. The shortcut sheet
reflects overrides. Stronger control/focus boundaries and reduced transition
animation are optional. Native accessibility uses egui's existing AccessKit
support, including named/value-bearing develop sliders. Full screen-reader
acceptance still requires the relevant OS assistive technology.

## Validation

- Workflow serialization/group selection and queue exclusive-owner/crash/retry
  tests pass. Invalid workflow versions cannot replace an existing file.
- An actual headless GPU render produced a 900×1197 16-bit PNG. The source hash
  stayed unchanged, and rendering over the source was refused.
- A saved workflow produced lossy WebP below its 150 KiB target. A two-task CLI
  queue completed the valid image and persisted a missing-file failure, returning
  status 1. Both metadata-bearing outputs met the size target.
- The actual app decode/develop/export worker generated a proof gallery. Browser
  checks confirmed image loading, selection/rating persistence across reload,
  slideshow open/close, and downloaded JSON with the matching receipt identity.
- Gallery tests cover escaping embedded captions, absence of private paths,
  destination preservation, mismatched receipt/ID rejection and invalid ratings.
- The final integrated checkpoint passed 517 workspace/all-target tests, zero
  failures and three documented manual-fixture ignores. Strict Clippy also passed
  with optional native codecs enabled. See [photography acceptance](photography-acceptance.md).
- Nine real egui dialogs/inspectors rendered successfully through offscreen WGPU
  in English and Dutch, including export, queue, workflow, gallery, print,
  customization, Detail, AI runtime and expert pixel shift. These verify layout
  rendering rather than every interactive branch. Dutch inspector search and
  static UI translation and dynamic template completeness checks also pass.
  Filenames and third-party technical diagnostics remain verbatim.

Native full-window screenshot capture remains unavailable on this host because
the compositor reports the surface as occluded. This did not prevent actual app
PDF/gallery export or headless GPU checks; it does prevent claiming a complete
visual acceptance pass. Browser snapshot capture also failed in the host tool;
the browser DOM/interactions above were exercised successfully.
