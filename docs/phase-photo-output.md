# Photography output expansion (2026-09-29)

This phase is part of `photography-roadmap.md`, not a claim that the complete
photography expansion is finished.

## Implemented

- Captions, keywords, creator, copyright, usage rights, contact email and chosen
  GPS information travel from the active photo or batch sidecar into exports.
  Capture tags, descriptions and GPS removal have separate controls. Runtime
  photo identity is excluded from saved export presets.
- JPEG, PNG, TIFF, WebP, AVIF, JPEG XL and Ultra HDR carry descriptive XMP.
  AVIF has native ICC and XMP items with rebuilt absolute offsets; Ultra HDR
  retains its required gain-map XMP and valid MPF offsets.
- ProPhoto RGB and custom RGB output ICC profiles use a 16-bit Little CMS
  transform after sizing/sharpening, with rendering intent and black-point
  compensation. Custom output profiles apply to SDR. HDR retains explicit
  Rec.2100 signaling; a conflicting SDR ICC is not embedded in PQ PNG/AVIF/JXL.
- Lossy WebP uses bundled libwebp with a quality slider; lossless remains
  available. Both preserve ICC/EXIF/XMP.
- File-size targets for JPEG, lossy WebP, AVIF and lossy JPEG XL include
  metadata overhead. The quality search preserves dimensions and respects a
  chosen quality floor. Failure retains the previous destination unchanged.
- Attribution text is a final-output watermark, not an editable document text
  layer. Text uses the bundled Ubuntu font, scales to fit, and has position,
  color, size and opacity controls. Watermark alpha blends in linear light and
  preserves untouched 16-bit samples; HDR marks use 203-nit diffuse white.
- The export dialog scrolls on smaller windows. The queue snapshots edits and
  descriptive metadata, supports selected recipes, pause after the current
  photo, retry and removal, and persists atomically. Restarted queues wait for
  Resume. Queue failures remain visible. Original photo paths are protected.
- Tools → Print / contact sheet creates ICC-tagged photographic PDFs with
  physical paper/margin/gap sizes, 1–10 rows/columns, fit/fill, captions and
  target resolution. Photos are rendered through the normal edit pipeline;
  the PDF writer holds one encoded photo at a time. The optional system viewer
  action opens the PDF for printing at actual size.
- Printer proofing accepts RGB/CMYK output and display profiles, rendering
  intent, black-point compensation and paper/ink simulation. LUT rebuilds run
  off the UI thread. The profile bytes remain available for later changes.

## Validation so far

- 58 `sp-export` tests passed at the integrated checkpoint, covering metadata policy, independent EXIF
  reading, native container signaling, 16-bit ICC round trips, size target
  success/failure, destination preservation, queue recovery and printing.
- Strict `sp-export` Clippy passed.
- The independent `lopdf` reader loads generated multi-page contact sheets;
  page count and physical A4 dimensions agree with the layout.
- All three standard proof tests passed. The explicit external-profile test
  also passed with macOS's Generic CMYK Profile, including gamut warning and
  different media simulation results. No physical printer output was measured.
- Independent libheif decoding passed for native AVIF ICC/XMP SDR and PQ HDR
  after metadata relocation (see raster input tests).
- An actual app export produced a 654 KiB photographic PDF from an edited JPEG;
  an independent system PDF preview displayed the expected photo and A4 layout.
  See [photography acceptance](photography-acceptance.md) for the final workspace
  checkpoint and native screen capture limitations; CLI/gallery acceptance is
  also recorded in `phase-photo-automation.md`.

## Practical limits

- Custom RGB output profiles cannot be used for Ultra HDR/PQ delivery; those
  have defined HDR encodings. CMYK profiles are used for proofing, while image
  output remains RGB.
- Text watermarks use the bundled font and basic glyph layout; complex-script
  shaping and custom fonts are not yet implemented. PDF filename captions use
  the built-in WinAnsi Latin font, including accented Latin letters, © and €,
  and replace unsupported characters with `?`. An independent PDF text extractor
  verifies those captions and literal parentheses/backslashes round-trip safely.
- Print PDFs embed high-quality 8-bit JPEG photos. They do not spool directly
  to a physical printer or promise a measured printer/paper match.
- A queue freezes edits/metadata but still requires its originals, selected
  profiles and models to remain available. Offline-pinned sources are supported.
  Pause lets an already running photo finish; it is not a mid-encoder pause.
