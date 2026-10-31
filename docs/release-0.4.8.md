# ShopPhoto 0.4.8 — file compatibility, clearer library states and audit fixes

This release widens what ShopPhoto opens, makes library and metadata states
easier to read, and fixes the issues found in a full repository audit. It
retains everything from 0.4.7.

## File compatibility

- HEIC, HEIF and AVIF open on every Mac build through macOS ImageIO, with
  thumbnails, embedded colour profiles, EXIF and orientation.
- XMP sidecars named `IMG.ARW.xmp` or with an uppercase `.XMP` extension are
  read.
- Finder "Open With" and launch arguments accept JPEG, PNG, TIFF, HEIC and
  AVIF as well as camera RAW files.
- Dropping several files opens the first and imports the rest.
- Fujifilm X-Trans RAF, linear DNG (for example ProRAW) and reduced-size ARW
  get a specific "not supported yet" message instead of a generic error.
- Exports write star ratings, rejects (rating −1) and colour labels as XMP.

## Interface

- Library tiles show "No preview" or "File missing" with the file type instead
  of a blank or endlessly loading tile; hovering explains why.
- Long file names are shortened in the middle so the extension stays visible.
- The empty state reads "Open your first photo" with a one-line format list.
- Caption and Keywords fields are labelled; the colour label picker shows
  swatches and names.
- The white balance row says whether the camera white balance is in use and
  offers "Reset to as shot" only when it has been changed.
- The folder sidebar uses folder and warning icons and equal-width actions.

## Fixes from the repository audit

- Automation no longer copies an older sidecar over a newer edit.
- **Exports never overwrite an existing file.** Exporting again to the same
  name creates `name (2).jpg`.
- Camera profiles, DCP files and lens profiles are validated before use.
- Model downloads are streamed (2 GiB cap) and activated atomically; downloads
  larger than 10 MB no longer fail.
- A damaged sidecar is set aside under a unique name and never replaces an
  earlier set-aside copy; sidecars from a newer schema version are refused
  instead of being overwritten.
- The folder watcher handles every queued event; the Adobe RGB export transfer
  curve is corrected.

## Validation

Workspace tests (619 passed, 13 ignored) and clippy pass. HEIC decoding was
checked with an iPhone HEIC file. Before/after screenshots of the changed
panels were compared. Lightroom and Capture One round trips, Finder
registration of the new formats and multi-file drop by hand have not been
exercised yet.
