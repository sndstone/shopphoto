# ShopPhoto 0.2.3 release audit

This is the evidence-backed closeout for `docs/improvement-plan.html`. A plan
item is marked done only when its implementation and repository-verifiable
acceptance are complete. Tests that require unavailable legal assets, hardware,
credentials or human visual judgement remain explicit release gates.

## Automated and local acceptance

- `cargo test --workspace`: passed (all workspace unit, integration, GPU golden
  and shader-contract tests).
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all --check` and `git diff --check`: passed.
- Real-camera decoder: five 101.9 MP X2D II 3FR files, two 51.4 MP X1D II 3FR
  files and one 51.4 MP Phocus FFF passed.
- Real-camera GPU pipeline: decode 48–334 ms, GPU open/render 18–62 ms and
  local-contrast rerender 3–4 ms across the eight files.
- Real 101.9 MP 2-by-2 Bayer export: 5828x4371 in 123 ms.
- M5 Pro strict synthetic budget: open 3 ms, proxy 1 ms, proxy plus NR 6 ms,
  1:1 NR tile 4 ms.
- The Mac created an `Rgba16Float` surface; this non-interactive harness could
  not drive the native window far enough to complete the delayed `--hdr-check`
  report, so the visual HDR gate remains open.
- Universal macOS packaging passed for arm64 and x86_64. The ad-hoc-signed app
  is 71 MB and the verified DMG is 53 MB, with no `.onnx`, `.mlmodelc` or
  `weight.bin` payloads. `dist/ShopPhoto-0.2.3.dmg` SHA-256:
  `73404fac84da0d3fa08ca9f6611f1616d02c56c5409b165668995267a0b302e8`.

## External or irrecoverable gates

| Item | What remains | Required input |
|---|---|---|
| A2 | Visual comparison of reconstructed blown sky/specular detail with Phocus | Human judgement and matched exports |
| E1 | An exact 0.2.2 source tag | Irrecoverable: that DMG's dirty source tree was never committed; do not fabricate provenance |
| E7 | M1 Pro, RTX 3060 and Ryzen iGPU measurements | Physical machines |
| F3 | Remove the vendored decoder | Upstream merges and release of [#837](https://github.com/dnglab/dnglab/pull/837), [#838](https://github.com/dnglab/dnglab/pull/838) and [#839](https://github.com/dnglab/dnglab/pull/839) |
| F4 | Mean holdout Delta E 2000 below 2.0 | Three to ten neutral 16-bit TIFF exports from Phocus |
| F5 | XDR and Android/Chrome viewing confirmation | Human visual session and an Ultra HDR-capable Android device |
| F6 | Signed/notarized tagged release | Apple Developer ID/notary secrets and Windows code-signing certificate |

Until F6 is supplied, 0.2.3 is source-complete but must not be published
as a production release.
