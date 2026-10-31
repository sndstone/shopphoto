#!/usr/bin/env python3
"""Offline AI cache/tile benchmarks with numerical parity checks.

Use the managed enhancement Python; --sr-model enables the local SwinIR check.
Timings exclude Rust rendering/IPC and compare warm inference on fixed pixels.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import statistics
import tempfile
import time

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
os.environ.setdefault("PYTORCH_ENABLE_MPS_FALLBACK", "1")

import numpy as np
from PIL import Image
import torch
import enhance


def synchronize(where):
    if where == "mps":
        torch.mps.synchronize()
    elif where == "cuda":
        torch.cuda.synchronize()


def measure(action, where, runs):
    times = []
    for index in range(runs + 1):
        synchronize(where)
        started = time.perf_counter()
        output = action()
        synchronize(where)
        if index:
            times.append(time.perf_counter() - started)
    return {"median_s": statistics.median(times), "samples_s": times}, output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", type=Path, default=Path(__file__).resolve().parents[1]/"docs/advanced-ai-features/ai-mask-scene.png")
    parser.add_argument("--sr-model", type=Path)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if not 1 <= args.runs <= 20:
        parser.error("--runs must be 1–20")
    where = enhance.device(torch)
    image = Image.open(args.image).convert("RGB")
    report = {"device": where, "torch": torch.__version__, "python": platform.python_version(),
              "size": list(image.size), "runs": args.runs, "revisions": enhance.REVISIONS,
              "scope": "warm Python inference/refinement/output; excludes Rust render and IPC", "results": []}

    def record(row):
        report["results"].append(row)
        print(json.dumps(row), flush=True)

    with tempfile.TemporaryDirectory(prefix="shopphoto-ai-benchmark-") as tmp:
        output = Path(tmp)/"mask.png"
        for kind in ("object", "depth"):
            request = {"input": str(args.image), "output": str(output), "kind": kind, "points": [[0.5, 0.5]]}
            def mask():
                enhance.mask(request)
                return np.asarray(Image.open(output)).copy()
            def fresh():
                enhance._PRODUCTS.clear()
                return mask()
            baseline, expected = measure(fresh, where, args.runs)
            cached, actual = measure(mask, where, args.runs)
            np.testing.assert_array_equal(expected, actual)
            record({"operation": kind, "fresh_image_inference": baseline, "cached_image_inference": cached,
                    "speedup": baseline["median_s"]/cached["median_s"], "max_u16_difference": 0})
            if kind == "object":
                coarse = actual.astype(np.float32)/65535

        raw_input = Path(tmp)/"input.rgba8"
        raw_input.write_bytes(image.convert("RGBA").tobytes())
        for kind in ("object", "depth"):
            def mask_input(path, raw=False):
                request = {"input": str(path), "output": str(output), "kind": kind, "points": [[0.5,0.5]]}
                if raw:
                    request.update(input_format="rgba8", width=image.width, height=image.height)
                enhance.mask(request)
                return np.asarray(Image.open(output)).copy()
            baseline, expected = measure(lambda: mask_input(args.image), where, args.runs)
            mapped, actual = measure(lambda: mask_input(raw_input, True), where, args.runs)
            np.testing.assert_array_equal(expected, actual)
            record({"operation": kind+"_input", "ordinary_image": baseline, "mapped_rgba8": mapped,
                    "speedup": baseline["median_s"]/mapped["median_s"], "max_u16_difference": 0,
                    "scope": "warm cached mask with input decoding; excludes input staging/encoding"})

        rgb = np.asarray(image)
        baseline, expected = measure(lambda: enhance.matte_boundaries(rgb, coarse, where, batch_size=1), where, args.runs)
        for batch_size in (2, 4):
            batched, actual = measure(lambda: enhance.matte_boundaries(rgb, coarse, where, batch_size=batch_size), where, args.runs)
            max_u16 = int(np.abs(np.round(expected*65535).astype(np.int32)-np.round(actual*65535).astype(np.int32)).max())
            if max_u16 > 2:
                raise AssertionError(f"Matting batch {batch_size} differs by {max_u16} alpha codes")
            record({"operation": "matting", "batch_size": batch_size, "single": baseline, "batched": batched,
                    "speedup": baseline["median_s"]/batched["median_s"], "max_u16_difference": max_u16})

        if args.sr_model:
            from spandrel import ModelLoader
            model = ModelLoader().load_from_file(args.sr_model).to(where).eval().float()
            if model.architecture.id != "SwinIR":
                parser.error("tile batch acceptance currently covers SwinIR only")
            proxy = image.copy()
            proxy.thumbnail((640, 384))
            rgb = np.asarray(proxy).astype(np.float32)/255
            baseline, expected = measure(lambda: enhance.tiled_sr(rgb, model, model.scale, batch_size=1), where, args.runs)
            for batch_size in (2, 4):
                batched, actual = measure(lambda: enhance.tiled_sr(rgb, model, model.scale, batch_size=batch_size), where, args.runs)
                max_u16 = int(np.abs(np.round(expected*65535).astype(np.int32)-np.round(actual*65535).astype(np.int32)).max())
                if max_u16 > 2:
                    raise AssertionError(f"SwinIR batch {batch_size} differs by {max_u16} output codes")
                record({"operation": "SwinIR", "size": list(proxy.size), "batch_size": batch_size,
                        "single": baseline, "batched": batched,
                        "speedup": baseline["median_s"]/batched["median_s"], "max_u16_difference": max_u16})

    if args.output:
        args.output.write_text(json.dumps(report, indent=2)+"\n", encoding="utf-8")


if __name__ == "__main__":
    main()
