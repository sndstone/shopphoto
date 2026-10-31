# AI performance review — 2026-10-10

The first practical improvements are retained interactive workers and image
embedding reuse. A Rust inference backend is feasible, but must be benchmarked
against a warm GPU baseline rather than the current repeated startup cost.
The original measurements below preceded implementation. Worker reuse, image
product caches and uncompressed GUI input are now implemented in the source;
see [the implementation record](phase-ai-performance.md). The existing 0.4.2
DMG predates those changes. Native enhancement inference remains a separate
feasibility prototype.

## Measured results

Local Apple M5 Pro, 24 GiB RAM, PyTorch 2.14.0, Metal/MPS. Offline models at
the revisions pinned in `scripts/enhance.py`; no model downloads or training.
Fixture: `docs/advanced-ai-features/ai-mask-scene.png` (1672 × 941), with a
512 × 288 thumbnail for the smaller check.

| Measurement | Baseline | Reuse | Scope |
| --- | ---: | ---: | --- |
| Text selection, 1672 × 941 | 2.6805 s | 0.51235 s | First request in fresh process vs median of two repeat requests |
| Text selection, 512 × 288 | 3.2409 s | 0.1491 s | Same protocol, separate fresh process |
| Point selection, 1672 × 941 | 0.82373 s | 0.34563 s | Median of three warm requests per phase; cached SAM image embedding prototype |
| SAM forward alone | 0.38929 s | 0.00476 s | Median of five warm runs; encoder + decoder vs decoder with retained embedding |

Text timings cover actual newline-JSON worker requests, processing, refinement,
16-bit PNG writing and response polling. They exclude Rust image rendering and
writing the input PNG. The first request includes cold process/library/model
initialization, but the files and Core ML system caches were not purged.
There is only one first-request observation at each image size.

The point prototype covers the complete Python mask function, including guided
refinement, ViTMatte and 16-bit PNG writing. It excludes Rust rendering and IPC.
Its output matched the baseline 16-bit mask exactly for every repetition of the
fixed image and point. SAM-only logits and IoU scores also matched exactly.
These are small local benchmarks, not latency guarantees for 50–150 MP photos,
other prompts, other hardware or a production cache implementation.

Temporary measurement scripts and raw results are in
`/tmp/shopphoto-ai-profile.py`, `/tmp/shopphoto-ai-profile-results.json`,
`/tmp/shopphoto-ai-cache-prototype.py` and
`/tmp/shopphoto-ai-cache-prototype-results.json` on the review machine.

## Concrete findings and implementation order

1. **Retain the interactive worker.** `app/enhance.rs::start_mask` spawns a
   thread for each action. `app/enhance_worker.rs` retains its Python process
   in thread-local storage, and drops/kills it at thread exit. Thus separate
   GUI mask actions reload libraries and models even though batch export
   already reuses its worker. Use a dedicated, serialized interactive service
   with bounded requests, cancellation, idle eviction and application shutdown
   cleanup. Keep error recovery and source/edit conflict checks.
2. **Reuse image-dependent work.** SAM currently encodes the image on every
   point request. Retain one bounded embedding for the canonical rendered
   image, invalidated by source/render revision, model revision and device.
   The temporary prototype achieved a 2.38× reduction in the full warm mask
   step on the fixture. A production cache must also prove correct invalidation
   when pixels change. Cache depth maps and semantic predictions separately
   so depth thresholds or people/sky choices can reuse the model output.
3. **Improve cache and tile scheduling.** The two-entry Python model cache
   holds segmentation + matting during repeated actions, but changing tool
   types evicts the segmentation model. Face and eye cascades also consume
   entries. Separate cheap detectors and use a measured memory allowance for
   neural models. Matting and super-resolution run tiles serially and read
   each output back to the CPU. Benchmark bounded tile batches, consistent
   tensor shapes and boundary-only processing on actual large photos. Test
   the effect of `empty_cache()` per request rather than removing memory
   controls without measurement. Draft preview quality can improve interaction
   latency, but final masks must retain the full-resolution refinement path.
4. **Prototype native inference for one model.** A Rust-controlled Core ML
   session is the first macOS candidate, following the existing RAW denoise
   architecture. Depth Anything V2 Small has a simpler contract than a
   prompted segmentation or arbitrary super-resolution checkpoint. Benchmark
   export/operation compatibility, compute-unit placement, load time, warm
   latency, memory and output quality. ONNX Runtime through Rust `ort` is a
   cross-platform candidate with hardware execution providers; it is a
   binding to a native engine. Candle is another Rust ML option, but support
   for each exact pinned architecture and its preprocessing must be verified.

## RAW denoise and model optimization

RAW denoise already uses retained Rust-owned Core ML sessions on macOS and
tract ONNX sessions elsewhere. The app has a persistent dedicated denoise
thread and reuses the unblended inference result when changing strength/mode.
A language rewrite would not address those already optimized paths. Further
work should measure tile inference, allocations/copies, overlap, hardware
placement and viewport contention.

Python tensor operations already execute native CPU/GPU kernels. Removing
Python can simplify packaging and reduce orchestration overhead, but faster
inference depends on the engine, graph and hardware. No Core ML/ONNX/Candle
conversion speedup was measured in this review.

Fine-tuning the same architecture principally changes quality; it does not
automatically reduce its computation. FP16, quantization, compression or
distillation into a smaller model may improve deployment cost. Validate hair,
thin boundaries, transparent objects, tile seams and HDR behavior. Camera RAW
denoise additionally requires CFA/color and camera-reference acceptance.
Compressed models can have unchanged or worse latency on some backends.

## Primary references

- [Hugging Face SAM: reusable image embeddings](https://huggingface.co/docs/transformers/v4.57.1/en/model_doc/sam).
- [Apple: measuring Core ML load, compilation and prediction](https://developer.apple.com/documentation/coreml/analyzing-a-core-ml-model-s-performance-in-xcode).
- [ONNX Runtime hardware execution providers](https://onnxruntime.ai/docs/execution-providers/index.html).
- [Rust ort execution provider integration](https://github.com/pykeio/ort/blob/main/docs/content/perf/execution-providers.mdx).
- [Hugging Face Candle](https://github.com/huggingface/candle).
- [Apple: compression performance varies by model and compute unit](https://apple.github.io/coremltools/docs-guides/source/opt-overview.html).
