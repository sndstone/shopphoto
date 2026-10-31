# ShopPhoto 0.4.2 — dynamic sidebar

The left sidebar in Develop now has **Presets**, **AI** and **Workflow** views.
Switch using the selector above the content; ShopPhoto remembers your choice
across restarts and in saved workspaces. AI puts local selections, camera RAW
denoise, Auto Balance/Auto Finish and model setup within reach. Workflow brings
related editing, delivery and workspace actions into the same space.

Existing presets and original-file protection are retained.
See [sidebar behavior and validation](phase-dynamic-sidebar.md) and the
[previous release](release-0.4.1.md) for JSONL/MCP and text-selection details.

The DMG contains a universal arm64/x86_64 app, six relocated native codec libraries,
an Applications shortcut and photography/research guidance. It is ad-hoc signed
for this local personal build. Apple Silicon native codecs require macOS 27.0;
the Intel slice retains standard codecs and its macOS 12.0 baseline. Compiling
the Intel slice does not establish physical Intel acceptance.

Optional Python selection/upscale models remain separately managed. Install by dragging ShopPhoto.app into
Applications; previous DMGs remain available.

Formatting and strict workspace/all-target Clippy with native codecs passed.
The full native-codec workspace suite passed 535 tests, zero failures and three
existing fixture/benchmark ignores. Sidebar pointer interactions and all three
views rendered offscreen in English and Dutch passed.

Final package verification and checksum are recorded in the repository release
notes and in the checksum file alongside the DMG.

## Final artifact verification

- Artifact: `dist/ShopPhoto-0.4.2-personal.dmg`, 702,515,777 bytes
  (670.0 MiB).
- SHA-256: `5b8007232521a31070f7707c5e19108e64e9fd16377af2e79ca8198b0513e0f8`. Matching `.dmg.sha256` file is beside it.
- DMG integrity, generated/mounted deep strict signatures, universal slices
  and version 0.4.2 passed. Both mounted documentation links are included.
- Mounted JPEG/HEIC/AVIF rendering and JSONL/MCP edit/export/preview/undo checks
  passed, preserving original hashes and sidecars as appropriate. All six
  native codec libraries loaded from the bundle without Homebrew runtime paths.
- The installed application and previous 0.4.0/0.4.1 DMGs remain in place.
  Intel was compiled and inspected; native Intel execution and full-window
  interaction remain separate acceptance checks.
