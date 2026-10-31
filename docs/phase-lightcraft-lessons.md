# Lightcraft lessons applied to ShopPhoto

Implemented on 2026-10-10 after reviewing
[Lightcraft at b70ae17](https://github.com/storytold/lightcraft/tree/b70ae1741e8236757377ca85eecaafb8788a82ab).
This is an implementation of the reviewed design ideas using ShopPhoto's
existing renderer, decoder, sidecars, catalog and optional enhancement runtime.
No Lightcraft source, models or branding was copied. Its
[MCP workflow](https://github.com/storytold/lightcraft/blob/b70ae1741e8236757377ca85eecaafb8788a82ab/docs/mcp.md)
informed the command interface. No external repository state was changed.

## Navigation and bounded memory

Opening a photograph replaces any pending open; generations discard stale
results, including reopening the same path. Decode workers prioritize opens,
then comparisons, then visible thumbnails, then adjacent previews. One worker
is reserved for foreground work. Thumbnail submissions are deduplicated and
the visible queue is capped at 512 pending jobs.

After opening a photo, the previous and next photos in the keyboard navigation
order have their embedded RAW previews prefetched. These are disposable camera
JPEG stand-ins, resized to at most 2048 pixels on the long edge, with an
8-entry LRU cache. They appear during decoding and are replaced by the normal
develop render. They can briefly show an older camera preview after an external
file replacement; full decoding remains authoritative. Raster neighbors are
not prefetched because their current preview API decodes the full float image.
Prefetch does not populate a full RAW/CFA cache or download an entire network
original.

`SHOPPHOTO_MEMORY_MB` defaults to 1024 MiB and is clamped to 256–8192 MiB.
Three quarters provides a byte-weighted decode working gate. One eighth each
bounds retained neighbor previews and thumbnail GPU textures, with each cache
capped at 128 MiB. The gate estimates RAW work from file bytes ×8 and raster
work from file bytes ×64, with a 128 MiB floor. Unknown sizes use 384 MiB.
An oversized decode runs alone; foreground waiters take priority over background
work. Thumbnail fallback releases its preview lease before requesting a full
decode lease.

These are scheduling estimates and cache limits, **not a process RSS ceiling**.
Active photographs, decoder temporaries, renderer textures, comparisons,
exports and AI models have other lifetimes. Real 100 MP navigation timing and
peak RSS across platforms remain measurement work; no new performance number
is claimed here.

## Shared commands, scripts and MCP

`sp-core::commands` validates a whole edit before applying it. Fifteen stable
scalar control IDs cover light, color, effects, white balance and sharpening;
commands also add radial/linear local masks, remove a mask or reset develop
settings. Each applied command creates one desktop undo/history step or one
headless session undo step. Unknown fields, unknown controls, nonfinite values,
out-of-range values and invalid mask geometry return errors without partial
edits. Existing slider handlers remain in place; the shared layer is deliberately
limited to these commands.

Tools → Command console accepts the versioned request below, plus
`engine.commands`, `develop.controls`, `photo.inspect`, `photo.undo` and
`photo.redo`. Desktop persistence uses the normal sidecar workflow. Opening,
explicit saving, ratings, flags, catalog queries and export commands are
available in the headless session rather than in this console.

```json
{"version":1,"command":"develop.set","params":{"values":{"light.exposure":0.3,"light.shadows":15}}}
```

Discover parameter schemas with `shopphoto commands`. A JSONL script contains
one request per line:

```jsonl
{"version":1,"command":"develop.set","params":{"values":{"light.exposure":0.3}}}
{"version":1,"command":"mask.radial","params":{"center":[0.5,0.5],"radius":[0.2,0.3],"feather":0.7,"exposure_ev":0.4}}
{"version":1,"command":"photo.rate","params":{"rating":4}}
{"version":1,"command":"export.render","params":{"path":"result.png","long_edge_px":1600}}
{"version":1,"command":"photo.save","params":{}}
```

```sh
shopphoto commands
shopphoto run input.dng --script edits.jsonl
shopphoto mcp
```

Scripts stop on the first failure, are limited to 8 MiB with 64 KiB lines,
and do not implicitly save. Earlier explicitly saved/exported results persist
if a later request fails. `photo.open` refuses to discard unsaved session edits.
`photo.save` checks the sidecar against the opened baseline before atomic save,
then updates catalog metadata and synchronizes a network sidecar. If those
post-save operations fail, the local sidecar is already saved. Undo includes
session ratings/flags, but the session's undo stack is not persisted as desktop
history. Original files and foreign XMP are preserved. `export.render` refuses
existing destinations and original paths and uses the normal GPU export path.

MCP uses newline-delimited JSON-RPC over stdio, protocol version `2025-06-18`.
Configure a client to launch the ShopPhoto executable with argument `mcp`.
The `execute` tool accepts the same versioned requests; start with
`engine.commands`, then `photo.open`. `render_preview` takes an empty arguments
object and returns an sRGB PNG image with current edits, crop and rotation,
at most 1024 pixels on its long edge. It does not write an output or save edits.
Rendering requires a compatible GPU; registry/inspection/edit operations do
not create a GPU device. This is a scoped photography interface, without
arbitrary filesystem or shell commands.

## Text-described local selections

Tools → AI masks includes “Describe a selection” and “Select
described objects”. Supply 1–8 comma-separated descriptions in at most 256
characters, for example `the red car, trees`. The worker combines CLIPSeg
probability coverage and uses the existing guided refinement/ViTMatte boundary
matting. Coverage is retained as a 16-bit local alpha mask. Existing cancellation,
stale-result checks and undo behavior apply. Weak matches return an error;
inspect the coverage and repair uncertain edges before relying on it.

Tools → AI runtime adds an optional text-model download checkbox.
For a fresh runtime:

```sh
python3 scripts/setup-enhance.py --text
```

For an already installed runtime, invoke its Python with
`scripts/enhance.py --download-text`. Inference only uses locally cached weights;
model download is explicit. The model is
[CIDAS/clipseg-rd64-refined](https://huggingface.co/CIDAS/clipseg-rd64-refined)
(Apache-2.0 model metadata), pinned to
`999e0328d9e10b484360c477313983f9afdd7050`, with safetensors weights. ViTMatte
uses the existing pinned model. This selection model is separate from the
RAW denoise runtime.

## GPU reliability and rendering checks

Desktop and headless defaults use Metal on macOS, DX12 on Windows and Vulkan
elsewhere. `SHOPPHOTO_GPU_BACKEND` overrides this (or `WGPU_BACKEND` if unset):
`metal`, `dx12`, `vulkan`, `gl`, comma-separated choices, or `auto`/`all`.
Unknown/disabled backends return errors. BUILD.md's fully GPU renderer remains
in place; a complete CPU fallback is not introduced.

Synchronous pipeline readbacks and staging remaps use checked map completion
and a 30-second GPU wait timeout. Finished photographic output requires opaque
alpha, and float output rejects NaN/infinity while preserving values above 1.
Coverage-mask readbacks allow partial alpha. Valid black photographs are accepted.

New synthetic GPU checks cover cached versus fresh rendering, SDR preview
versus 16-bit export, windowed versus full export, and a CPU tone reference
including scene-linear HDR headroom. Fifteen edit cases exercise tone, color,
white balance, detail, curves, local contrast, masks, grain and vignette. These
are reference/equivalence checks, not a full independent CPU develop pipeline.

## Validation

- Formatter, strict workspace/all-target Clippy (also with native codecs) and
  full workspace tests pass.
- 528 workspace tests passed, zero failed; three existing tests require explicit
  fixtures or a manual benchmark and remain ignored.
- The optional Python suite passed 13 tests with four unrelated fixture tests
  skipped, including actual CLIPSeg + ViTMatte inference on the installed runtime.
- Ten egui dialogs/inspectors rendered offscreen through WGPU in English and
  Dutch, including the new command console and text-model installer options.
- Executable CLI/MCP checks exercised discovery, grouped edits, masks, ratings,
  export, explicit sidecar save, invalid-script rejection, undo/redo and PNG
  preview, including a cropped and rotated preview. Original hashes and sidecar
  preservation were verified.

Validation on this host uses Apple Silicon/Metal. Native full-window interaction,
Windows/DX12, Linux/Vulkan and physical HDR display acceptance remain distinct
checks. The follow-up [0.4.1 personal package](release-0.4.1.md) includes these
changes; its release notes record package verification and OS requirements.
