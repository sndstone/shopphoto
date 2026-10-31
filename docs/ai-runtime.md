# Native AI and optional Python runtime

Version 0.4.6 adds production Rust/Core ML subject selection, promptable object
masks, ViTMatte boundary refinement and faithful SwinIR 2×/4× crop enlargement.
The personal macOS package includes validated native bundles and runs these
paths without Python on macOS 15+. **AI runtime** lists native availability.
See [model sources, licensing and native behavior](native-ai-models.md) and
[0.4.6 validation](release-0.4.6.md). Camera denoise remains separate.

## Optional portable and additional-model runtime

Open **AI runtime** in the top bar to check installed Python packages, the
selected CPU/CUDA/Metal backend, and locally cached object, depth, semantic and matting
models. Readiness checks and inference are offline. The check distinguishes a
missing or broken interpreter from missing packages and missing model files.

**Install runtime and selected models** runs the separate enhancement installer
with the Python 3.11+ executable you enter. It downloads Python dependencies.
Model downloads are individually opted into; model-card/license links are shown
before selecting them. The SwinIR option verifies the checkpoint SHA-256.
An existing environment is upgraded in place after a Python interpreter update.
Custom `SHOPPHOTO_ENHANCE_PYTHON` environments must be maintained by their owner;
the managed installer does not overwrite them. The RAW denoise installer
remains separate.

Interactive masks use one app-owned serial Rust worker. Separate actions reuse
its isolated Python process and models; two minutes idle releases the process
and GPU allocations. Cancellation, inference failures and application shutdown
kill and reap the worker. Pending actions are bounded and shutdown cancels the
active action. Source generations and saved-edit comparisons discard results
when the photograph is reopened or edited during inference.

Batch exports retain one isolated Python worker per export thread until that
thread exits. Workers use newline JSON requests, atomic response files and no
shell execution. Each retains at most four neural models within a default
768 MiB parameter/buffer allowance. `SHOPPHOTO_ENHANCE_CACHE_MB` adjusts this
allowance (128–2048 MiB); oversized models execute without being retained.
This bounds retained weights, not total RSS or activation/allocator memory.
Multiple export workers have separate allowances. Face/eye cascades use a
separate small cache.

Image products have a separate 64 MiB/four-entry allowance and reset when
proxy pixels or dimensions change. SAM image embeddings, depth predictions
and semantic predictions reuse inference for changed point prompts, depth
thresholds and people/sky choices. Keys include model revision and device;
filenames do not determine image identity. Full-resolution edge refinement
still uses the current original RGB pixels on every request.

GUI masks pass validated, memory-mapped RGBA8 pixels rather than compressing
and decoding an intermediate PNG. Width, height, file length and the 150 MP
limit are checked before allocating RGB. Render readback and temporary GPU
buffers are released before Python inference. This uses more temporary disk
space than compressed PNG input. Ordinary image requests remain supported.

Version 0.4.3 also retains one canonical input file between point/threshold/tool
changes, avoiding repeated GPU development and file staging. The retained file
is bounded to 512 MiB by default (`SHOPPHOTO_MASK_INPUT_CACHE_MB`, 64–1024 MiB).
Pixel-changing edits, reopened sources, resolved profiles and applied camera
denoise invalidate it; failure/cancellation, idle and shutdown release it.
Decoded RAW/RGB buffers and temporary renderers are released before inference.

On Apple Silicon, the optional neural models use the Apple GPU through PyTorch
MPS, with CPU fallback for unsupported operators. OpenCV detectors and some
refinement/file work use CPU. Camera RAW denoise is separate: Core ML permits
CPU, GPU and the Neural Engine by default and chooses supported placement.
This is not a guarantee of NPU execution. See
[hardware placement and performance changes](phase-performance-implementation.md).

Matting/SwinIR can process 1–4 consecutive tiles of identical dimensions via
`SHOPPHOTO_ENHANCE_TILE_BATCH`. Default is one: local batching benchmarks did
not establish a useful default improvement. HAT/DRCT keep single-image calls.
No padding or halo changes are introduced to form batches; Float32, existing
HDR handling and full-resolution refinement remain the reference paths.

Local upscale checkpoint size/mtime changes reload the network.
Core ML and ONNX RAW denoise likewise retain at most two sessions
per worker thread; changes to the model bundle's file sizes/mtimes invalidate
the cached session. No model is shared concurrently across worker threads.

See [implementation and validation](phase-ai-performance.md), the offline
`scripts/benchmark-enhance.py` benchmark and the separate fixed-shape
`scripts/prototype-coreml-depth.py` feasibility probe. Core ML conversion is
now enabled for the listed subject and enlargement models. The older depth
probe remains a development experiment; depth and text selection still use the
optional Python backend. Conversion tools are not installed-app dependencies.

## Element selection and face-detail assistance

The AI sidebar and Tools → AI masks share **Select element** with Sky, People,
Cars and Motorcycles. **Create adjustment layer** creates a named, selected,
undoable local adjustment with full-resolution alpha and its overlay enabled.
Sky creates one continuous selection. People, cars and motorcycles first show
individual previews; choose one or several to create independent layers.
Use Local & retouch controls to affect only that mask. Include/exclude points
can refine the selected layer while retaining its edits and undo history.
Empty, nonfinite or misaligned generated results are rejected without creating
an empty layer. Missing weights produce a runtime-setup error. The old generic
sky-at-point button is removed to avoid confusing a point-prompt object with
semantic sky detection.

Native automatic instances use SSDLite COCO boxes followed by SlimSAM; a semantic
category query can merge two people and is not treated as an instance detector.
The Python fallback uses the same detector/segmenter family. Inspect edges and
use the available repair tools; pixel-perfect coverage is not guaranteed.

Native sky uses the MIT OneFormer ADE20K Swin Tiny model. The optional Python
sky fallback retains [MaskFormer Swin Tiny ADE20K](https://huggingface.co/facebook/maskformer-swin-tiny-ade),
pinned to revision `ea43bbf72d30ee4278deb93bc3d8bd2856c4c6ec`; its
[noncommercial license](https://github.com/facebookresearch/MaskFormer/blob/main/LICENSE)
is identified by a separate installer choice. MaskFormer weights are not included
in the 0.4.6 native package. Native people/vehicles use COCO detector classes
rather than this semantic model. Missing or insufficient-confidence predictions
produce a visible no-match result and no empty layer.

Object selection accepts up to 32 foreground/exclusion points. At least one
foreground point is required. Points and masks use canonical pre-lens coordinates.
Library's optional face/eye-detail analysis uses OpenCV's shipped Haar detectors
and regional edge detail. It provides advisory face/eye counts and scores;
it does not recognize identities, diagnose closed eyes or change ratings.

## Validation and environment

Rust session reuse/revision/eviction, Python persistent-worker protocol and
numerical tile/refinement tests pass. All eleven Python acceptance tests passed,
including actual SlimSAM foreground/exclusion prompts, ViTMatte, Depth Anything,
MaskFormer people/sky, OpenCV face/eye analysis and SwinIR SDR/PQ HDR inference
on Metal. An explicitly run Rust acceptance test also passed the real persistent
worker, 16-bit alpha import and saved-sidecar roundtrip. That fixture-dependent
test remains ignored in the default suite.

The development environment exposed a real upgrade bug: its generic Python
executables still referenced a removed Homebrew interpreter after the versioned
executable had been refreshed. The managed installer now replaces all three
POSIX entry points atomically. Running the installer with Python 3.14.7 repaired
the environment and installed SciPy, required by MaskFormer. The pinned semantic
weights (approximately 167 MB) were downloaded into the development model cache
for evaluation, outside the repository.


Camera RAW denoise has a separate saved device policy in **Tools → Settings… →
Camera RAW denoise**: Automatic, GPU + CPU, Neural Engine (NPU) + CPU, or CPU only.
Core ML can use CPU fallback, so Neural Engine selection excludes GPU without
promising every layer runs on the NPU. The setting applies to interactive RAW,
batch Auto and GUI exports; other Python AI tools keep their current backend.
Changing the policy invalidates RAW inference and session caches. See
[implementation and validation](phase-live-preview-compute.md).

## Crop-specific upscale entry point

The crop HUD includes **AI upscale crop…**. It commits the current crop and
opens Export at its original working size so the initial size recommendation
cannot downsample it before AI. The installed SwinIR 2× model is selected when
available, and an already selected model is preserved. In Export, choose a
supported native 2×/4× checkpoint or install the optional SwinIR model. The
existing upscale path runs after crop/geometry and before output sharpening,
watermarks and borders. Current input support remains sRGB SDR or Rec.2020 PQ
HDR, with a 150 MP output ceiling. Direct scale choices, exact final-size control
and native inference are planned in the linked implementation record.
