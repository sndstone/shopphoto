"""Attach complete upstream license text and conversion provenance to a bundle."""
from pathlib import Path
import shutil


def attach_native_notices(bundle, metadata):
    model_id = metadata["id"]
    if model_id.startswith("swinir-"):
        owner = "swinir"
    elif model_id.startswith("slimsam-"):
        owner = "slimsam"
    elif model_id.startswith("maskformer-"):
        owner = "maskformer"
    elif model_id.startswith("oneformer-"):
        owner = "oneformer"
    elif model_id.startswith("vitmatte-"):
        owner = "vitmatte"
    elif model_id.startswith("ssdlite-"):
        owner = "torchvision"
    else:
        raise ValueError("No verified upstream license notice for this model")
    license_root = Path(__file__).resolve().parents[1]/"assets/ai-licenses"
    shutil.copyfile(license_root/(owner+"-LICENSE.txt"), bundle/"LICENSE.txt")
    notice = (f"ShopPhoto native enhancement model: {model_id}\n"
              f"Source: {metadata['source']}\n"
              f"Pinned revision or checkpoint SHA-256: {metadata['revision']}\n"
              f"Upstream license: {metadata['license']} (full text in LICENSE.txt)\n\n"
              "Modified by ShopPhoto: converted original pretrained weights into a\n"
              "fixed-shape or explicitly bounded-shape Core ML graph. No fine tuning\n"
              "is applied. Model preprocessing and measured\n"
              "conversion differences are recorded in manifest.json. Upstream authors\n"
              "do not endorse ShopPhoto or these conversions.\n")
    if owner == "maskformer":
        notice += ("\nMeta/Facebook MaskFormer ADE20K pretrained weights are explicitly\n"
                   "NONCOMMERCIAL (CC-BY-NC-4.0). This optional personal-use bundle\n"
                   "must not be represented as commercially licensed.\n")
    if owner == "torchvision":
        notice += ("\nThe BSD-3-Clause text covers Torchvision code. Torchvision states\n"
                   "pretrained models may carry additional dataset-derived terms:\n"
                   "https://github.com/pytorch/vision#pre-trained-model-license\n"
                   "These are official COCO-trained SSDLite320 MobileNetV3 weights.\n"
                   "Dataset rights need separate review before redistribution.\n")
    (bundle/"NOTICE.txt").write_text(notice, encoding="utf-8")
