# ShopPhoto 0.4.1 — personal macOS build

This build includes the [Lightcraft lessons implementation](phase-lightcraft-lessons.md)
on top of the [0.4.0 photography workflows](release-0.4.0.md): prioritized RAW
navigation, bounded neighbor previews and thumbnail caches, shared undoable
develop commands, JSONL/MCP automation, optional text-described local selections,
checked GPU readbacks and rendering equivalence checks.

Tools → Command console accepts versioned JSON edit commands. Tools → AI masks
adds text-described selections; install their optional model through Tools →
AI runtime. The Python runtime and text-selection weights are managed separately
and are not embedded in the DMG. On this Mac they are already installed and
tested. The app embeds the updated worker/installer scripts.

```sh
/Applications/ShopPhoto.app/Contents/MacOS/ShopPhoto commands
/Applications/ShopPhoto.app/Contents/MacOS/ShopPhoto run input.dng --script edits.jsonl
/Applications/ShopPhoto.app/Contents/MacOS/ShopPhoto mcp
```

The package is a universal arm64/x86_64 app. Native HEIC/AVIF
input is included in the Apple Silicon slice and requires macOS 27 or later;
the Intel slice retains the standard raster codecs and macOS 12 baseline.
The newer bundled codec libraries raise the Apple Silicon requirement from
0.4.0's macOS 26 minimum; the package declares their measured minimum.
Ad-hoc signing is used for this local build. Intel compilation does not establish physical Intel acceptance.

The DMG includes an Applications shortcut, Photography Guide and Research
Notebook. Install by dragging ShopPhoto.app into Applications. The previous
0.4.0 DMG remains available alongside this release.

## Verification

Formatting and strict workspace/all-target Clippy with native codecs passed.
The complete native-codec workspace suite passed 532 tests, zero failures and
three existing explicit fixture/benchmark ignores.

Package integrity, signatures, universal architectures, version, relocated codec dependencies and mounted CLI/MCP rendering
are checked before delivery. Final artifact details are recorded in the
repository release notes and checksum file alongside the DMG. The source validation is documented in the linked implementation
notes; full-window and other-platform acceptance remain separate checks.

## Final artifact verification

- Artifact: `dist/ShopPhoto-0.4.1-personal.dmg`, 703,444,286 bytes
  (670.9 MiB).
- SHA-256: `d876158eb4aecdcd0b623ec3a0eaddcb677e978424d028af1af245965a149bf8`. Matching `.dmg.sha256` file is beside it.
- DMG integrity and deep/strict signature verification passed for both the
  generated and read-only mounted app. The build is ad-hoc signed.
- Universal arm64/x86_64 slices and version 0.4.1 verified. The Apple Silicon
  codec slice declares macOS 27.0; Intel retains the macOS 12.0 baseline.
- All six codec libraries are relocated into the app. Mounted JPEG/HEIC/AVIF
  renders produced valid 385×512 PNGs; loader tracing confirmed bundled codec
  paths and no Homebrew runtime paths. Source hashes remained unchanged.
- Mounted executable discovery, JSONL edits/masks/rating/export/save, rejected
  invalid edits, MCP preview and undo/redo passed. Cropped/rotated preview
  dimensions and source/sidecar preservation were verified.
- The previous `ShopPhoto-0.4.0-personal.dmg` remains available. This task did
  not replace the installed application. Intel was compiled and inspected;
  native Intel execution and full-window interaction were not tested here.
