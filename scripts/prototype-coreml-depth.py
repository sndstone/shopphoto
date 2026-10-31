#!/usr/bin/env python3
"""Offline fixed-shape Core ML depth feasibility probe, separate from the app.

Requires coremltools in an evaluation environment and the pinned cached model.
Exports full Float32 without changing weights; compares Core ML with Metal.
It is not an installer or a production backend and never downloads models.
"""
import argparse
import json
import os
from pathlib import Path
import statistics
import time
import types

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", type=Path, default=Path(__file__).resolve().parents[1]/"docs/advanced-ai-features/ai-mask-scene.png")
    parser.add_argument("--output", type=Path, required=True, help="new .mlpackage outside existing model directories")
    parser.add_argument("--report", type=Path)
    parser.add_argument("--runs", type=int, default=5)
    args = parser.parse_args()
    if args.output.exists() or args.output.suffix != ".mlpackage":
        parser.error("--output must be a new .mlpackage path")
    if not 1 <= args.runs <= 20:
        parser.error("--runs must be 1–20")
    import coremltools as ct
    import numpy as np
    import torch
    from transformers import AutoImageProcessor, AutoModelForDepthEstimation
    import enhance
    if not torch.backends.mps.is_available():
        parser.error("this comparison requires macOS with Metal/MPS")
    torch.set_num_threads(6)
    processor = enhance.pretrained(AutoImageProcessor, enhance.DEPTH, use_fast=False)
    model = enhance.pretrained(AutoModelForDepthEstimation, enhance.DEPTH, use_safetensors=True).eval()
    image = enhance.load_rgb({"input": str(args.image)})
    image.thumbnail((2048,2048))
    pixels = processor(images=image, return_tensors="pt")["pixel_values"]

    class Depth(torch.nn.Module):
        def __init__(self, model):
            super().__init__()
            self.model = model

        def forward(self, pixels):
            return self.model(pixel_values=pixels).predicted_depth

    wrapper = Depth(model).eval()
    with torch.inference_mode():
        expected = wrapper(pixels).numpy()
        # DinoV2's bicubic positional resize depends only on the fixed shape.
        # Precompute its exact result; substituting bilinear would alter quality.
        layer = model.backbone.embeddings
        patch_size = model.config.backbone_config.patch_size
        height, width = pixels.shape[-2:]
        dummy = torch.empty((1, (height//patch_size)*(width//patch_size)+1, layer.position_embeddings.shape[-1]))
        positions = layer.interpolate_pos_encoding(dummy, height, width)
        layer.register_buffer("export_positions", positions)
        def positions_for_shape(self, embeddings, height, width):
            return self.export_positions
        layer.interpolate_pos_encoding = types.MethodType(positions_for_shape, layer)
        torch.testing.assert_close(wrapper(pixels), torch.from_numpy(expected), rtol=0, atol=0)
        traced = torch.jit.trace(wrapper, pixels, strict=False, check_trace=False)

    started = time.perf_counter()
    converted = ct.convert(traced,
                           inputs=[ct.TensorType(name="pixels", shape=tuple(pixels.shape), dtype=np.float32)],
                           outputs=[ct.TensorType(name="depth", dtype=np.float32)],
                           convert_to="mlprogram", minimum_deployment_target=ct.target.macOS15,
                           compute_precision=ct.precision.FLOAT32)
    converted.save(str(args.output))
    conversion_s = time.perf_counter()-started
    coreml_times = []
    cpu_pixels = pixels.numpy()
    for index in range(args.runs+1):
        started = time.perf_counter()
        actual = converted.predict({"pixels": cpu_pixels})["depth"]
        if index:
            coreml_times.append(time.perf_counter()-started)
    wrapper = wrapper.to("mps")
    device_pixels = pixels.to("mps")
    mps_times = []
    with torch.inference_mode():
        for index in range(args.runs+1):
            torch.mps.synchronize()
            started = time.perf_counter()
            mps_result = wrapper(device_pixels)
            torch.mps.synchronize()
            if index:
                mps_times.append(time.perf_counter()-started)
        mps_result = mps_result.cpu().numpy()

    def normalized(depth):
        lo, hi = np.percentile(depth, [1,99])
        return np.clip((depth-lo)/max(float(hi-lo), 1e-6), 0, 1)

    if not np.isfinite(actual).all():
        raise ValueError("Core ML returned nonfinite depth")
    report = {"coremltools": ct.__version__, "torch": torch.__version__, "model": enhance.DEPTH,
              "revision": enhance.REVISIONS[enhance.DEPTH], "input_shape": list(pixels.shape),
              "convert_and_load_s": conversion_s, "runs": args.runs,
              "coreml_median_s": statistics.median(coreml_times), "mps_median_s": statistics.median(mps_times),
              "max_cpu_difference": float(np.max(np.abs(actual-expected))),
              "max_mps_difference": float(np.max(np.abs(actual-mps_result))),
              "max_normalized_mps_difference": float(np.max(np.abs(normalized(actual)-normalized(mps_result)))),
              "coreml_samples_s": coreml_times, "mps_samples_s": mps_times,
              "scope": "unintegrated fixed-shape Float32 model forward, excludes preprocessing/refinement and Rust"}
    print(json.dumps(report, indent=2))
    if args.report:
        args.report.write_text(json.dumps(report, indent=2)+"\n", encoding="utf-8")


if __name__ == "__main__":
    main()
