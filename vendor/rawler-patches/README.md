# rawler patches and upstream status

`vendor/rawler` is rawler 0.7.2 with three local changes, wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`. These patch files are the
same changes isolated against the pristine crates.io source, so they can be
sent to [dnglab](https://github.com/dnglab/dnglab) — where the rawler crate
lives under `rawler/` — without hand-extracting them from the vendored tree.

Apply to a dnglab checkout:

```bash
git apply /path/to/shopphoto/vendor/rawler-patches/*.patch
```

Regenerate after touching `vendor/rawler` (the upstream source is whatever
cargo unpacked into the registry cache):

```bash
UP=~/.cargo/registry/src/*/rawler-0.7.2
diff -u "$UP/src/decoders/tfr.rs" vendor/rawler/src/decoders/tfr.rs
```

## The patches

**0001 — Hasselblad X2D II 100C model aliases.** Upstream has no definition
for the 2025 body. The X2D 100C sensor definition and color matrix are a
close stand-in; a dedicated matrix would come from an Adobe DCP or a Phocus
Match profile. Adding aliases is the smallest change that makes the camera
decode at all.

**0002 — Hasselblad X1D II 50C alias for Phocus files.** Phocus writes the
make-prefixed model string (`Hasselblad X1D II 50C`) where the camera writes
the bare one, so the same body fails to match depending on which program
last wrote the file.

**0003 — honor the raw IFD's TIFF Compression tag in the .3FR/.fff decoder.**
The upstream decoder decides compressed-vs-uncompressed purely from the
per-camera `uncompressed` hint. Phocus-processed `.fff` files from these
cameras are LJPEG-compressed even though their `.3FR` originals are not, so
the hint sends the decoder down the uncompressed path and it panics on a
short slice. The patch reads the Compression tag when present and falls back
to the existing hint behavior when it is absent, so no currently-working
camera changes behavior.

**0004 — return an error instead of `todo!()` in `Value::get_string`.** A
file that declares a non-ASCII type for a tag the decoders read as a string
(Make, on the `get_decoder` path) reaches an unimplemented arm and panics
inside library code. The signature already returns `Result`; this returns the
error. Found by fuzzing.

**0005 — a truncated APP1 segment hangs the JFIF parser.** On a failed
`App1::read_segment`, the loop `continue`s with the marker variable still
`0xFFE1` and the reader unmoved, so it re-enters the same arm forever. Five
bytes (`FF D8 FF E1 33`) are enough. The patch stops parsing and returns the
segments collected so far — the segment length is what failed to read, so
there is nothing to skip past. Found by fuzzing.

Patches 3, 4 and 5 are the ones worth upstreaming first: all are crash or hang fixes on
malformed input rather than new-camera support, all are independent of the
alias patches, and none can change behavior for a file that decodes today.

## Current upstream status

The live changes were rebased onto dnglab 0.8.0 at commit `ae01bcb2` and tested
against its rawler unit suite on 2026-09-03:

- X2D II dedicated camera definition: [dnglab #837](https://github.com/dnglab/dnglab/pull/837)
- Phocus FFF compression and model alias: [dnglab #838](https://github.com/dnglab/dnglab/pull/838)
- Truncated APP1 infinite-loop fix and regression: [dnglab #839](https://github.com/dnglab/dnglab/pull/839)
- Non-ASCII `Value::get_string`: already fixed upstream by commit `3740e41b`.

ShopPhoto's TIFF allocation preflight remains at the application boundary.
Current upstream explicitly declines a safe-untrusted-input guarantee, and a
previous broad hardening proposal was closed; copying a second partial TIFF
walker into rawler would not be a sound upstream patch.

Drop `vendor/rawler`, the `[patch.crates-io]` entry, and this directory after
the open PRs merge and a compatible rawler release ships.
