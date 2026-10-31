# ShopPhoto 0.5.0 — ShopPhoto RAW Denoise

ShopPhoto now ships its own neural RAW denoiser. **Detail → Noise Reduction →
ShopPhoto RAW Denoise** runs `r4_plain` from ShopPhoto's own
[rawdenoise-lab](https://github.com/sndstone/rawdenoise-lab) project on the
Bayer sensor data before demosaic, for every RAW camera ShopPhoto decodes
(Hasselblad, Canon, Nikon, Sony, DNG and the rest):

- **Cleanest** uses the complete model result; **Preserve detail** keeps
  20–25 % of the original micro-contrast. **Off** returns to the manual
  luminance and chroma controls. Before/after and split views work as before.
- The model is bundled in the macOS app. Builds without it offer
  **Download ShopPhoto model…**, which fetches the checksum-pinned public
  release and compiles it on the Mac.
- A 50 MP X1D II frame denoises in about 1.7 s and a 100 MP X2D II frame in
  about 3.3 s on Apple Silicon.

ShopPhoto's tiling, feathering and stuck-photosite pre-pass match the
`rawdenoise` reference tool: on its three demo DNGs the denoise changes
correlate at 0.99997–0.99999 with the reference output. The model was trained
for the X1D II; its model card is included beside these notes.

## Faster launch

Opening ShopPhoto no longer waits while it looks for memory cards. The card
search used to list every mounted volume, including network shares and the
startup disk, before the window appeared — 30–40 s on a Mac with SMB shares
mounted. It now skips network shares and the startup disk and runs in the
background, so the window opens immediately; Import from card shows
"Looking for cards…" while it searches. Camera cards are still detected.

## Other changes

- Core ML models with a single input of any name and strided or Float16 output
  are read without per-value conversion.
- Windows and Linux use tract 0.23 for ONNX inference.

## macOS download

`ShopPhoto-0.5.0.dmg` contains a universal app for Apple Silicon and Intel with
the ShopPhoto RAW Denoise model, the photography guide and research notebook.
This build is ad-hoc signed, not Apple-notarized; macOS may require explicit
approval to open it.
