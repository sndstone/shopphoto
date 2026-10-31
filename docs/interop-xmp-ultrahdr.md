# Interop: XMP ratings and Ultra HDR export

Status: complete (2026-07-26). These answer two of the four open questions in
BUILD.md §9.

## XMP sidecars for ratings and flags

`.spedit` stays the source of truth — it carries the full `EditState`, which
no XMP schema expresses. XMP mirrors annotations and descriptive metadata that other applications
read and write.

New XMP sidecars use Adobe's extension-replaced `IMG_0001.xmp` convention.
Reading also recognizes full-name `IMG_0001.3FR.xmp` and uppercase `.XMP`.
Saving retains an existing convention. If both exist, extension-replaced wins
and the ambiguity is logged. `.spedit` always appends (`IMG_0001.3FR.spedit`).

| ShopPhoto | XMP |
|---|---|
| rating 0–5 | `xmp:Rating="0".."5"` |
| `Flag::Reject` | `xmp:Rating="-1"` (Adobe's reject convention) |
| `Flag::Pick` | `shopphoto:Flag="Pick"` |

Picks have no cross-vendor XMP representation — Lightroom keeps them in its
catalog — so they live in our own namespace, where they round-trip with us
and are ignored elsewhere. Rejects use `-1` because that is the one
convention every reader understands.

Reading accepts Lightroom's attribute form and Bridge's element form, matches
on namespace URI and local name so any namespace prefix works, and returns `None` rather
than an error for foreign or malformed XMP. That last part is a correctness
requirement, not politeness: a camera-metadata-only packet must never be read
as "rating 0" and silently reset a photo.

Writing is **opt-in** (a checkbox in the Library metadata panel, persisted in
preferences), because creating files inside someone's photo folders is their
decision. Reading is always on — both the folder scan and single-file open
adopt an existing XMP rating when we have no sidecar of our own, so photos
rated elsewhere arrive with their stars.

Color labels are mirrored as `xmp:Label` using Red, Yellow, Green, Blue and
Purple. Captions, keywords/hierarchies, creator, copyright, rights, contact
and GPS are also supported. Custom/localized label names have limited fidelity.
Foreign develop/crop edits are preserved as XML but are not applied.

## Ultra HDR JPEG export

AVIF was the only HDR output, and it is not what phones and browsers display.
An Ultra HDR JPEG is a baseline SDR JPEG carrying a second, smaller JPEG —
the gain map — plus metadata describing how to combine them:

```text
HDR = (SDR + offset_sdr) * 2^(gain * (max - min) + min) - offset_hdr
```

Decoders that know the format (Android, iOS 18+, Chrome, Safari) reconstruct
the highlights; every other decoder shows the SDR rendition and ignores the
trailing bytes. The extension stays `.jpg` for exactly that reason.

**Gain map** (`sp-export/src/gainmap.rs`): single channel, from the luminance
ratio between the SDR and PQ renditions of the same edit. PQ is absolute and
the SDR rendition is relative, so the two are tied together by BT.2408's
203 cd/m² reference white — which is what makes a scene with no highlights
above SDR white produce a flat, zero map rather than an arbitrary offset.
Half resolution (decoders upsample it; a 100 MP export should not carry a
100 MP second image), boost clamped to 4 stops. `GainMapParams::apply` is the
decoder's published formula, so the round-trip is tested rather than assumed.

**Container** (`sp-export/src/ultrahdr.rs`): both discovery mechanisms are
written, because real decoders use both.

- **MPF** (CIPA Multi-Picture Format), an APP2 segment holding a small TIFF
  IFD with each image's size and offset. Offsets are relative to the MP
  header, *not* the file, and the primary's own offset is always 0.
- **GContainer XMP**, an APP1 directory of items (`Primary`, then `GainMap`
  with its byte length).

The MPF offsets depend on the primary's final length, which depends on the
MPF segment — so the segment has a fixed size, is emitted with placeholders,
and is patched once the length is known. Tests walk the segment chain and
check that the recorded offset lands exactly on the gain map's SOI and that
the two recorded sizes account for the whole file.

The base image is always encoded 4:4:4: chroma subsampling would smear
highlight edges across the gain map's own edges. The map is encoded at
quality ≥ 90 regardless of the base quality, because blocking artifacts in a
gain map read as halos.

**Cost**: the export worker renders the edit twice. The format picker says so.

Measured end-to-end on a synthetic clipped highlight: the PQ render carries
1.33 stops of headroom over the SDR one, concentrated in the highlight half
of the frame. macOS ImageIO reads a produced sample as an ordinary sRGB JPEG
with its ICC profile intact — the SDR-fallback property that matters most.

Still unverified: whether a real HDR display shows the boost. That needs a
phone or a browser looking at a file, not a test.
