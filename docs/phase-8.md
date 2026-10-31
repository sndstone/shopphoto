# Phase 8 — Performance and robustness

- Asynchronous histogram mapping removes the known UI-thread GPU wait.
- CFA orientation rotation is row-parallel with rayon and skips normal orientation.
- F12 overlay reports frame time, approximate texture residency, render count, catalog size, and undo depth.
- Full export uses bounded GPU tiles and checks cancellation between tiles.
- Interactive proxy resolution follows at most twice the physical viewport edge instead of a fixed size.
- The ~200 MB quarter-resolution f16 intermediate is released after proxy construction and recreated only for a 2× superpixel export.
- `fuzz/fuzz_targets/raw_decode.rs` is the cargo-fuzz harness for malformed DNG/RAW input.
  Run 2026-07-26 with the real X2D II / X1D II headers as corpus seeds. It found two
  out-of-memory classes, both inside the decoder's TIFF layer, both now rejected up front
  by `sp_raw::preflight` before rawler sees the file:
  1. A truncated file that still advertises the original's ~200 MB strip byte counts.
  2. A mutated field count (1.2 billion LONGs) that makes rawler's IFD reader allocate
     4.9 GB before reading anything.
  3. The same allocation reached through fields whose TYPE is garbage (type 0). A strict
     type table skipped those, so unknown types now count as one byte — the smallest any
     type can be — and are checked like every other.
  4. and 5. The same allocation reached by files the check did not recognize as TIFF at
     all: `II*'` instead of `II*\0`, then a magic of 0x81. `rawler`'s reader takes the
     byte-order marker and *ignores the magic* — its `if magic != 42` branch is commented
     out upstream — so the guard now keys on `II`/`MM` alone. **A guard in front of a
     parser must accept exactly what that parser accepts; anywhere the two disagree is a
     hole.** Revisit that line first if the decoder is swapped or upstream restores its
     magic check.
  6. A PANIC, not an OOM: `rawler::formats::tiff::Value::get_string` hits a `todo!()`
     when a tag the decoders read as a string declares a non-ASCII type. sp-raw's
     contract is that malformed input never panics, so this is fixed twice — the
     vendored rawler returns the error its signature already allows (patch 0004, worth
     upstreaming), and `decode_impl` wraps the decoder in `catch_unwind` for the panic
     paths not yet found. Note that `fuzz/` is its own workspace: it needed its own
     `[patch.crates-io]`, or it fuzzes the crates.io rawler while the app runs the
     vendored one.
  7. A 63-byte file whose single IFD's next-IFD pointer points back at itself. rawler
     follows the chain without a bound, inserting entries into a map until memory runs
     out. Terminating our own walk was not enough — a cyclic chain describes infinitely
     many images, so it is now rejected outright. Sub-IFD pointers are only skipped, not
     rejected: two parents referencing one child is strange but not self-contradictory.
  8. A HANG, in a different container path entirely: `rawler::formats::jfif::Jfif::parse`
     loops forever on a truncated APP1 segment, because the error arm `continue`s without
     advancing the reader or the marker. Five bytes (`FF D8 FF E1 33`) suffice. Fixed in
     vendor/rawler (patch 0005). The pre-flight check does not and should not cover this:
     the file is not TIFF, and a guard that tried to validate every container rawler
     probes would be a second decoder.
  9. An oversized field reached through a sub-IFD the guard did not follow. Fixed by
     walking exactly the pointer tags the decoder walks (`SubIFDs`, `ExifIFDPointer`,
     `IccProfile`, `GPSInfo` — verbatim from `wellknown_sub_ifd_tags`; keep them in sync).

**Where this stops, deliberately.** Fuzzing still reaches the same allocation through
rawler's offset-*correction* paths, where maker-note IFDs are parsed with a signed
adjustment applied to every offset, so a pointer that reads as out-of-range to a plain
walker lands inside the file for the decoder. Matching that would mean reimplementing
rawler's offset arithmetic in the guard — the second decoder this file argues against
one paragraph up, with a second decoder's bugs and, worse, the chance of rejecting a real
maker note.

The pre-flight check therefore covers the *plausible* corruption a photographer actually
meets — truncated copies, half-written files, bad cards — and does not claim to cover
adversarial input. The residual case needs a deliberately crafted file, produces an
out-of-memory abort rather than memory corruption, and crashes an editor the user pointed
at the file themselves. Closing it properly means bounding the decoder's allocations
inside rawler (worth an upstream issue), or decoding in a subprocess under an rlimit —
not a bigger guard. `fuzz/artifacts/raw_decode/` keeps the reproducers.
  The guard is two invariants that no real file can violate — no strip, tile or field
  value block may be longer than the file containing it, and no image may exceed a
  gigapixel — plus a bounded IFD walk. Both artifacts now decode-fail in 1–3 ms, and the
  105 MP / 51 MP sample files still decode unchanged. A file that is not classic TIFF, or
  cannot be walked, is passed straight through: this is a guard against absurdity, not a
  validator.
- Sidecar corruption quarantine and atomic writes remain covered by tests.

Measured on the development Apple Silicon system: X2D II 105 MP decode was 141 ms, GPU open/render 67 ms, and the exact 26.3 MP high-precision 2× export render 222 ms. Cross-machine M1 Pro / RTX 3060 / Ryzen iGPU acceptance and a one-hour fuzz run require those physical machines and are release-QA tasks, not properties CI can simulate.

## 0.2.3 acceptance rerun — 2026-09-03

The current M5 Pro system passed the strict synthetic tier: open 3 ms, proxy
1 ms, proxy with NR 6 ms and 1:1 NR tile 4 ms. Eight locally owned Hasselblad
files (five 101.9 MP X2D II 3FR, two 51.4 MP X1D II 3FR and one 51.4 MP
Phocus FFF) decoded in 48–334 ms, opened/rendered on the GPU in 18–62 ms and
rerendered local contrast in 3–4 ms. A real X2D II 2-by-2 Bayer export rendered
5828x4371 pixels in 123 ms.

This adds one current Apple Silicon data point. It does not substitute for the
named M1 Pro, RTX 3060 and Ryzen iGPU acceptance machines.
