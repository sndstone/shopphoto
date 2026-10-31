# Native enhancement conversion

The offline development scripts `scripts/export-native-ai.py` and
`scripts/export-native-selection.py` create the actual Core ML bundles used by
the native macOS enhancement path. Python is an export dependency for these
graphs, not an application runtime dependency. The scripts read pinned cached
safetensors or explicit trusted local weights; they never download weights or
execute remote model code. RAW denoise uses a separate runtime.

Use an isolated environment with the enhancement requirements and Core ML
Tools. These exports were checked with Torch 2.14.0, Transformers 4.57.6,
Spandrel 0.4.2 and Core ML Tools 9.1.dev1 on Apple Silicon macOS 27. Core ML
Tools warns that Torch 2.14 is newer than its tested baseline. Every export
must pass actual reference parity before it is installed. A fresh setup should
prefer a Python/Torch combination supported by its Core ML Tools release.

Example commands, with `python` referring to that isolated environment:

```sh
NATIVE_ROOT="$HOME/Library/Application Support/shopphoto/enhance-models/native"
python scripts/export-native-ai.py semantic --output "$NATIVE_ROOT/maskformer-ade"
python scripts/export-native-ai.py oneformer --output "$NATIVE_ROOT/oneformer-ade"
python scripts/export-native-ai.py matte --output "$NATIVE_ROOT/vitmatte-small"
python scripts/export-native-ai.py upscale --checkpoint "$HOME/Library/Application Support/shopphoto/enhance-models/SwinIR-M-x2.pth" --output "$NATIVE_ROOT/swinir-2x"
python scripts/export-native-ai.py upscale --checkpoint "$HOME/Library/Application Support/shopphoto/enhance-models/SwinIR-M-x4.pth" --output "$NATIVE_ROOT/swinir-4x"
python scripts/export-native-selection.py detector --output "$NATIVE_ROOT/ssdlite-coco"
python scripts/export-native-selection.py sam-encoder --output "$NATIVE_ROOT/slimsam-encoder"
python scripts/export-native-selection.py sam-box --output "$NATIVE_ROOT/slimsam-box"
python scripts/export-native-selection.py sam-points --output "$NATIVE_ROOT/slimsam-points"
```

Each bundle contains `manifest.json` and `model.mlmodelc`. The manifest records
named shapes, preprocessing, source revision, license, Float32 precision,
minimum macOS 15, SHA-256 of every compiled file and measured conversion parity.
Complete upstream license text and a source/conversion notice are included and
hashed. The checked-in `assets/ai-licenses` directory supplies these notices.
The optional `--package` argument on `export-native-ai.py` keeps an editable
source `.mlpackage` outside the runtime bundle. Existing output paths are
refused. A temporary sibling is renamed only after validation succeeds.

Tensor contracts use Float32 input and output buffers:

- `oneformer-ade`: `pixels [1,3,512,512]` →
  `class_logits [1,150,151]`, `mask_logits [1,150,128,128]`. Same preprocessing
  and ADE class indices as MaskFormer below; the semantic task token is fixed
  inside the graph. This is the permissively licensed semantic option.
- `maskformer-ade`: `pixels [1,3,512,512]` →
  `class_logits [1,100,151]`, `mask_logits [1,100,128,128]`. Resize preserving
  aspect ratio so the longest side is 512, normalize RGB with ImageNet mean
  `[.485,.456,.406]` and std `[.229,.224,.225]`, then pad right/bottom with zero
  in normalized space. Unpad masks before restoring source dimensions. Classes
  sky/person/car/motorcycle are 2/12/20/116; class 150 is no-object.
- `vitmatte-small`: `pixels [1,4,512,512]` → `alpha [1,1,512,512]`.
  First three planes use ImageNet normalization; the last is trimap `0/.5/1`.
  A 512-pixel boundary tile includes a 64-pixel halo. Fixed-shape export freezes
  the exact input-independent bicubic absolute position interpolation and
  inlines relative position interpolation; it does not substitute another
  interpolation method.
- `swinir-2x`: `pixels [1,3,192,192]` → `rgb [1,3,384,384]`.
- `swinir-4x`: `pixels [1,3,192,192]` → `rgb [1,3,768,768]`.
  Both consume RGB sRGB floats in `[0,1]`; model mean stays inside the graph.
  Restored RGB can overshoot that range and is clamped during final image
  quantization. `tile:192` includes a 32-pixel halo, leaving a 128-pixel useful core.
- `ssdlite-coco`: `pixels [1,3,320,320]` → decoded boxes and 91-class logits.
  Resize directly to a square and normalize `(rgb/255-.5)/.5`. Boxes are in
  320-pixel image coordinates. The host clips boxes, applies softmax and NMS,
  then rescales to source coordinates. COCO person/car/motorcycle are 1/3/4.
- `slimsam-encoder`: ImageNet-normalized, aspect-preserved, padded
  `pixels [1,3,1024,1024]` → `embeddings [1,256,64,64]`.
- `slimsam-box`: embeddings plus `boxes [1,1,4]` in resized 1024-pixel space →
  `mask_logits [1,1,3,256,256]`, `iou_scores [1,1,3]`. Choose the highest-IoU
  mask; resize logits to 1024, unpad, restore source dimensions, then sigmoid.
- `slimsam-points`: embeddings plus `points [1,1,N,2]` and `labels [1,1,N]`,
  with **real point count N from 1 to 32**, returns the same logits and IoU
  outputs. Labels are Float32 0/1 and cast to integer inside the graph. The
  manifest's `flexible_axes` records bounds for axis 2. No artificial padding
  prompts are added. Conversion was checked at counts 1, 2, 3 and 32. An initial
  fixed-32 padded graph changed the selected mask substantially and was rejected.

On the checked-in scene and a half-intensity tensor, maximum absolute
native/reference differences were about `0.00133` for semantic mask logits
(`0.000239` mean), `0.0000552` for alpha, and `0.00000120` for SwinIR RGB.
These are conversion checks against identical CPU Torch weights, not a mask
quality or super-resolution quality benchmark. The first four compiled bundles
total approximately 383 MiB before compression. Semantic parity used Core ML
CPU-only; matte/SwinIR used `ALL`, which permits system-selected compute units
and does not prove Neural Engine placement. Each manifest records its result.

OneFormer was exported from pinned revision
`05f2812b1eccf9909b3897777450f8d68148cafc`, loaded with `weights_only=True`.
Its deformable attention was algebraically restructured into per-level
operations to avoid Core ML's rank-6 tensor limitation. The rewritten Torch
forward is checked against the unmodified model before tracing. A constant-only
product translation handles fixed spatial shapes and rejects nonconstant input.
Core ML semantic coverage parity is checked for all four classes on two image
tensors: maximum sky coverage difference `0.000134`, mean `0.000000170`;
maximum class posterior difference `0.000309`. Raw near-zero logits can have
larger relative error, so the OneFormer gate checks the actual normalized
semantic coverage rather than relying on relative logit error. Its compiled
bundle is approximately 282 MiB before compression. This verifies conversion;
photographic selection quality still needs the separate end-to-end fixtures.

Core ML Tools with Python 3.14 exhibited a background `_Py_Dealloc` crash when
an ephemeral NumPy view was released after prediction. The scripts retain owning
NumPy buffers until model teardown. The native Objective-C host owns its
`MLMultiArray` and does not use the Python bridge.

The cached [MaskFormer ADE checkpoint](https://huggingface.co/facebook/maskformer-swin-tiny-ade)
is **CC-BY-NC-4.0**. Conversion preserves this restriction. Its bundle remains
explicitly optional and noncommercial, including in personal packages. Availability
in a local cache does not imply commercial or redistribution rights.
[ViTMatte](https://github.com/hustvl/ViTMatte) is MIT,
[SwinIR](https://github.com/JingyunLiang/SwinIR) and
[SlimSAM](https://huggingface.co/Zigeng/SlimSAM-uniform-77) are Apache-2.0,
and [Torchvision](https://github.com/pytorch/vision/blob/main/LICENSE) is BSD-3-Clause.
Training dataset rights should be reviewed separately before redistribution.
The official classical DF2K SwinIR-M x4 checkpoint hash is
`4e78e33f22c1aa8a773db0cf4a7381bae97c2362c717f155439ebc690cbd9215`.

[OneFormer Tiny](https://huggingface.co/shi-labs/oneformer_ade20k_swin_tiny)
is available as a verified MIT native semantic bundle. [Apple's SAM 2 Tiny conversion](https://huggingface.co/apple/coreml-sam2-tiny)
is a future prompt-refinement alternative; it is not required by the SlimSAM
path implemented here. See the
[broader subject-tool plan](phase-native-subject-tools.md).
