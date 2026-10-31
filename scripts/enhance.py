#!/usr/bin/env python3
"""ShopPhoto enhancement worker. No denoise code, weights or directories are used.

Install in a separate Python 3.11+ environment:
  python -m pip install -r scripts/enhance-requirements.txt
  python scripts/enhance.py --download-masks
Set SHOPPHOTO_ENHANCE_PYTHON to that environment's Python executable.
Requests are JSON files; inference never executes a shell or remote model code.
"""
import argparse
import hashlib
import json
import mmap
import os
import sys
from collections import OrderedDict
from pathlib import Path

SAM = "Zigeng/SlimSAM-uniform-77"
DEPTH = "depth-anything/Depth-Anything-V2-Small-hf"
MATTE = "hustvl/vitmatte-small-composition-1k"
SEMANTIC = "facebook/maskformer-swin-tiny-ade"
TEXT = "CIDAS/clipseg-rd64-refined"
REVISIONS = {
    TEXT: "999e0328d9e10b484360c477313983f9afdd7050",
    SAM: "79c09c1ce6b4ae51f00634ed171d9b8e888f6911",
    DEPTH: "5426e4f0f36572d16453bbda7a8389317b1bef99",
    MATTE: "6a58ad7646403c1df626fbd746900aec7361ea1d",
    SEMANTIC: "ea43bbf72d30ee4278deb93bc3d8bd2856c4c6ec",
}

# ADE20K names the motorcycle category "minibike, motorbike". Match whole
# aliases, never substrings ("car" must not match "skyscraper").
SEMANTIC_SUBJECTS = {
    "semantic_sky": ("sky",),
    "semantic_people": ("person", "people"),
    "semantic_cars": ("car",),
    "semantic_motorcycles": ("motorcycle", "motorbike", "minibike"),
}


def semantic_class_index(labels, kind):
    aliases = SEMANTIC_SUBJECTS[kind]
    matching = [int(index) for index, label in labels.items()
                if set(part.strip().casefold() for part in label.split(",")) & set(aliases)]
    if len(matching) != 1:
        raise ValueError(f"Semantic model has no unique {aliases[0]} class")
    return matching[0]


def quantize_mask(alpha, size):
    """Do not report success for a nonfinite, misaligned or empty AI result."""
    import numpy as np
    if alpha.shape != (size[1], size[0]) or not np.isfinite(alpha).all():
        raise ValueError("AI returned invalid mask pixels or dimensions")
    pixels = np.round(np.clip(alpha, 0, 1)*65535).astype(np.uint16)
    if not np.any(pixels):
        raise ValueError("AI could not create a usable mask. Try an include point or a different selection; no empty layer was created")
    return pixels

def setting_int(name, default, low, high):
    try:
        return max(low, min(high, int(os.environ.get(name, default))))
    except (ValueError, TypeError):
        return default


def product_bytes(value):
    if isinstance(value, (tuple, list)):
        return sum(product_bytes(item) for item in value)
    if hasattr(value, "numel") and hasattr(value, "element_size"):
        return value.numel() * value.element_size()
    return int(getattr(value, "nbytes", 0))


def model_bytes(value):
    if isinstance(value, (tuple, list)):
        return sum(model_bytes(item) for item in value)
    if hasattr(value, "parameters") and hasattr(value, "buffers"):
        return sum(product_bytes(tensor) for tensor in (*value.parameters(), *value.buffers()))
    # Spandrel descriptors own a torch module, unlike HF's processor/model pair.
    if hasattr(value, "model"):
        return model_bytes(value.model)
    return 0


class MemoryCache:
    """Bound retained tensor/weight bytes and entry count, not inference peak RSS."""
    def __init__(self, limit_bytes, max_entries, measure=product_bytes):
        self.limit_bytes, self.max_entries, self.measure = limit_bytes, max_entries, measure
        self.entries = OrderedDict()
        self.retained_bytes = 0

    def __len__(self):
        return len(self.entries)

    def __contains__(self, key):
        return key in self.entries

    def clear(self):
        self.entries.clear()
        self.retained_bytes = 0

    def get(self, key, loader):
        if key in self.entries:
            entry = self.entries.pop(key)
            self.entries[key] = entry
            return entry[0]
        result = loader()
        size = self.measure(result)
        if size > self.limit_bytes or self.max_entries < 1:
            return result
        while self.entries and (len(self.entries) >= self.max_entries or self.retained_bytes + size > self.limit_bytes):
            _, (_, removed_bytes) = self.entries.popitem(last=False)
            self.retained_bytes -= removed_bytes
        self.entries[key] = (result, size)
        self.retained_bytes += size
        return result


# Weight allowance is per process; workspace/activations/allocator caches are
# additional. An oversized model runs without being retained. Idle GUI workers
# are killed by Rust, releasing Python and Metal/CUDA allocations as well.
_MODELS = MemoryCache(setting_int("SHOPPHOTO_ENHANCE_CACHE_MB", 768, 128, 2048) * 1024**2, 4, model_bytes)
_PRODUCTS = MemoryCache(64 * 1024**2, 4)
_IMAGE_KEY = None
_DETECTORS = {}


def cached_model(key, loader):
    return _MODELS.get(key, loader)


def model_key(name, where):
    return name, REVISIONS[name], where


def image_key(proxy):
    global _IMAGE_KEY
    # Pixel content, not filename/mtime: GUI requests use different temp files,
    # and changing pixels in an existing path must invalidate image inference.
    key = (proxy.size, hashlib.sha256(proxy.tobytes()).digest())
    if key != _IMAGE_KEY:
        _PRODUCTS.clear()
        _IMAGE_KEY = key
    return key


def load_rgb(request):
    """Read ordinary images or the GUI's validated uncompressed RGBA8 input."""
    from PIL import Image
    if request.get("input_format") == "rgba8":
        width, height = request.get("width"), request.get("height")
        if any(type(value) is not int or value <= 0 for value in (width, height)) or width*height > 150_000_000:
            raise ValueError("Invalid RGBA8 dimensions or input exceeds 150 MP")
        with open(request["input"], "rb") as source:
            if os.fstat(source.fileno()).st_size != width*height*4:
                raise ValueError("RGBA8 input size does not match its dimensions")
            # Conversion owns the RGB pixels before the mapping is closed;
            # there is no additional Python copy of the RGBA input file.
            with mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as pixels:
                return Image.frombuffer("RGBA", (width,height), pixels, "raw", "RGBA", 0, 1).convert("RGB")
    if request.get("input_format") not in (None, "image"):
        raise ValueError("Unknown mask input format")
    with Image.open(request["input"]) as source:
        if source.width*source.height > 150_000_000:
            raise ValueError("Image input exceeds 150 MP")
        return source.convert("RGB")


def status():
    """Inspect packages and cached snapshots without downloading model files."""
    import importlib.metadata
    packages = {}
    missing = []
    for name in ("torch", "torchvision", "transformers", "spandrel", "opencv-python-headless", "Pillow", "numpy", "scipy"):
        try:
            packages[name] = importlib.metadata.version(name)
        except importlib.metadata.PackageNotFoundError:
            missing.append(name)
    models = {}
    try:
        from huggingface_hub import snapshot_download
        for name in REVISIONS:
            try:
                snapshot = Path(snapshot_download(name, revision=REVISIONS[name], local_files_only=True))
                models[name] = (snapshot/"config.json").is_file() and (snapshot/"preprocessor_config.json").is_file() and any(snapshot.glob("*.safetensors"))
            except Exception:
                models[name] = False
    except ImportError:
        models = {name: False for name in REVISIONS}
    models["torchvision/ssdlite320-mobilenet-v3-large-coco"] = detector_path().is_file() if "torch" not in missing else False
    backend = "Unavailable"
    error = None
    if not missing:
        try:
            import torch
            import cv2
            import spandrel
            import transformers
            backend = device(torch)
        except Exception as failure:
            error = str(failure)
    return {"python": sys.version.split()[0], "packages": packages, "missing_packages": missing,
            "models": models, "device": backend, "error": error, "ready": not missing and error is None}


def pretrained(cls, name, *, download=False, **kwargs):
    return cls.from_pretrained(name, revision=REVISIONS[name], local_files_only=not download, **kwargs)


def device(torch):
    requested = os.environ.get("SHOPPHOTO_ENHANCE_DEVICE", "auto")
    if requested != "auto":
        return requested
    if torch.cuda.is_available():
        return "cuda"
    if torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def refine_alpha(rgb, coarse):
    """Full-resolution edge-guided refinement in bounded, overlapping regions.

    This improves alignment, but is not a learned hair/transparency matte.
    Imported professional alpha mattes remain available for difficult edges.
    """
    import cv2
    import numpy as np
    h, w = rgb.shape[:2]
    coarse = cv2.resize(coarse.astype(np.float32), (w, h), interpolation=cv2.INTER_LINEAR)
    out = np.empty((h, w), dtype=np.float32)
    radius, halo, tile = 8, 32, 1024
    for y in range(0, h, tile):
        for x in range(0, w, tile):
            y0, x0 = max(0, y-halo), max(0, x-halo)
            y1, x1 = min(h, y+tile+halo), min(w, x+tile+halo)
            guide = cv2.cvtColor(rgb[y0:y1, x0:x1], cv2.COLOR_RGB2GRAY).astype(np.float32) / 255
            p = coarse[y0:y1, x0:x1]
            mean = lambda v: cv2.boxFilter(v, -1, (2*radius+1, 2*radius+1), borderType=cv2.BORDER_REFLECT)
            mi, mp = mean(guide), mean(p)
            a = (mean(guide*p)-mi*mp) / (mean(guide*guide)-mi*mi+0.0004)
            refined = np.clip(mean(a)*guide + mean(mp-a*mi), 0, 1)
            # Preserve certain interiors; avoid moving an entire selected region.
            boundary = (p > 0.001) & (p < 0.999)
            boundary = cv2.dilate(boundary.astype(np.uint8), np.ones((17, 17), np.uint8)) > 0
            result = np.where(boundary, refined, p)
            hh, ww = min(tile, h-y), min(tile, w-x)
            out[y:y+hh, x:x+ww] = result[y-y0:y-y0+hh, x-x0:x-x0+ww]
    return out


def semantic_alpha(outputs, class_index):
    """Class-weighted query coverage, normalized against all foreground classes.

    Do not classify sky by brightness or people by skin color. The final query
    class is the no-object category and contributes no semantic coverage.
    """
    import torch
    classes = outputs.class_queries_logits.float().softmax(-1)[..., :-1]
    masks = outputs.masks_queries_logits.float().sigmoid()
    if not 0 <= class_index < classes.shape[-1]:
        raise ValueError("Semantic class is not present in the selected model")
    numerator = torch.einsum("bq,bqhw->bhw", classes[..., class_index], masks)
    total = torch.einsum("bq,bqhw->bhw", classes.sum(-1), masks)
    return (numerator / total.clamp(min=1e-6)).clamp(0, 1)


DETECTOR_URL = "https://download.pytorch.org/models/ssdlite320_mobilenet_v3_large_coco-a79551df.pth"
INSTANCE_BACKEND = "python-ssdlite-a79551df-slimsam-79c09c1c-v1"
DETECTOR_CLASSES = {"semantic_people": 1, "semantic_cars": 3, "semantic_motorcycles": 4}


def detector_path():
    import torch
    return Path(torch.hub.get_dir())/"checkpoints"/DETECTOR_URL.rsplit("/",1)[-1]


def load_detector(download=False):
    import torch
    from torchvision.models.detection import ssdlite320_mobilenet_v3_large
    path = detector_path()
    if not path.is_file():
        if not download:
            raise ValueError("Individual element detector is unavailable. Open Tools → AI runtime and install the mask models")
        path.parent.mkdir(parents=True,exist_ok=True)
        torch.hub.download_url_to_file(DETECTOR_URL,str(path),hash_prefix="a79551df",progress=False)
    # Torchvision's pinned official checkpoint. Do not permit arbitrary pickle
    # objects or any implicit downloads during inference.
    if not hashlib.sha256(path.read_bytes()).hexdigest().startswith("a79551df"):
        raise ValueError("Individual element detector checksum mismatch; reinstall mask models")
    model = ssdlite320_mobilenet_v3_large(weights=None, weights_backbone=None)
    model.load_state_dict(torch.load(path,map_location="cpu",weights_only=True))
    return model.eval()


def detector_candidates(prediction, kind, size):
    """COCO instance boxes are independent of ADE semantic category unions."""
    import numpy as np
    width,height = size
    candidates=[]
    for index,(label,score,box) in enumerate(zip(prediction["labels"].tolist(),prediction["scores"].tolist(),prediction["boxes"].tolist())):
        if label != DETECTOR_CLASSES[kind] or not np.isfinite(score) or score < 0.45:
            continue
        if len(box)!=4 or not np.isfinite(box).all():
            continue
        x0,y0,x1,y1=np.clip(box,[0,0,0,0],[width,height,width,height])
        if x1-x0<2 or y1-y0<2:
            continue
        candidates.append({"id":index,"confidence":float(score),"bounds":[x0/width,y0/height,(x1-x0)/width,(y1-y0)/height],"box":[x0,y0,x1,y1]})
        if len(candidates)==16:
            break
    return candidates


def object_instances(request, image, proxy, pixels_key, where):
    if request.get("selected_ids") is not None and request.get("selected_backend") != INSTANCE_BACKEND:
        raise ValueError("Selection model changed; detect elements again before creating layers")
    import torch
    from torchvision.transforms.functional import to_tensor
    from transformers import SamProcessor, SamModel
    if request["kind"] not in DETECTOR_CLASSES:
        raise ValueError("Individual element detection supports people, cars and motorcycles")
    detector_key=("ssdlite320", "a79551df", "cpu")
    detector=cached_model(detector_key,load_detector)
    with torch.inference_mode():
        def predict_boxes():
            result=detector([to_tensor(proxy)])[0]
            return tuple(result[name].detach().cpu() for name in ("boxes","scores","labels"))
        predictions=_PRODUCTS.get((pixels_key,detector_key,"boxes"),predict_boxes)
    prediction=dict(zip(("boxes","scores","labels"),predictions))
    candidates=detector_candidates(prediction,request["kind"],proxy.size)
    if not candidates:
        raise ValueError("No confidently detected individual elements; use a point selection or painted mask")
    selected=request.get("selected_ids")
    if selected is not None:
        available={item["id"] for item in candidates}
        if (not isinstance(selected,list) or not selected or any(type(value) is not int or value not in available for value in selected) or len(selected)!=len(set(selected))):
            raise ValueError("Selected elements no longer match this image; detect elements again")
        candidates=[item for item in candidates if item["id"] in selected]
    key=model_key(SAM,where)
    processor,model=cached_model(key,lambda:(pretrained(SamProcessor,SAM),pretrained(SamModel,SAM,use_safetensors=True).to(where).eval()))
    for item in candidates:
        def predict():
            inputs=processor(proxy,input_boxes=[[item["box"]]],return_tensors="pt")
            inputs={k:v.to(device=where,dtype=torch.float32 if v.is_floating_point() else v.dtype) for k,v in inputs.items()}
            with torch.inference_mode():
                embeddings=_PRODUCTS.get((pixels_key,key,"sam"),lambda:model.get_image_embeddings(inputs["pixel_values"]))
                outputs=model(image_embeddings=embeddings,**{k:v for k,v in inputs.items() if k!="pixel_values"})
                masks=processor.image_processor.post_process_masks(outputs.pred_masks.cpu(),inputs["original_sizes"].cpu(),inputs["reshaped_input_sizes"].cpu(),binarize=False)[0]
                best=int(outputs.iou_scores[0,0].argmax())
                return masks[0,best].sigmoid().numpy()
        item["alpha"]=_PRODUCTS.get((pixels_key,key,"instance",item["id"]),predict)
        item.pop("box")
    save_instances(request,image,candidates,where)


def save_instances(request, image, candidates, where):
    """Preview proposals first; only chosen IDs allocate full-resolution mattes."""
    import numpy as np
    from PIL import Image
    if request["kind"] == "semantic_sky":
        raise ValueError("Sky is a continuous selection; use the sky mask action")
    if not candidates:
        raise ValueError("No confidently detected individual elements; use a point selection or painted mask")
    selected = request.get("selected_ids")
    if selected is not None:
        available = {item["id"] for item in candidates}
        if (not isinstance(selected, list) or not selected
                or any(type(value) is not int or value not in available for value in selected)
                or len(selected) != len(set(selected))):
            raise ValueError("Selected elements no longer match this image; detect elements again")
        if image.width*image.height*2*len(selected) > 600_000_000:
            raise ValueError("Selected masks exceed the 600 MB selection budget; create fewer layers at a time")
        candidates = [item for item in candidates if item["id"] in selected]
    destination = Path(request["output"])
    rgb = np.asarray(image) if selected is not None else None
    manifest = {"version": 1, "backend": INSTANCE_BACKEND, "width": image.width, "height": image.height, "candidates": []}
    for item in candidates:
        query = item["id"]
        coarse = item.pop("alpha")
        if selected is None:
            x,y,width,height=item["bounds"]
            margin=max(width,height)*0.08
            bounds=(max(0.,x-margin),max(0.,y-margin),min(1.,x+width+margin),min(1.,y+height+margin))
            box=(int(bounds[0]*image.width),int(bounds[1]*image.height),max(1,int(np.ceil(bounds[2]*image.width))),max(1,int(np.ceil(bounds[3]*image.height))))
            preview=image.crop(box).convert("RGBA")
            preview.thumbnail((160,160))
            coarse_image=Image.fromarray(np.round(coarse*255).astype(np.uint8))
            alpha=coarse_image.crop((int(bounds[0]*coarse_image.width),int(bounds[1]*coarse_image.height),max(1,int(np.ceil(bounds[2]*coarse_image.width))),max(1,int(np.ceil(bounds[3]*coarse_image.height))))).resize(preview.size,Image.Resampling.BILINEAR)
            preview.putalpha(alpha)
            name = f"candidate-{query}.png"
            preview.save(destination.parent/name)
            item["preview"], item["mask"] = name, None
        else:
            alpha = matte_boundaries(rgb, refine_alpha(rgb, coarse), where)
            name = f"mask-{query}.png"
            Image.fromarray(quantize_mask(alpha, image.size)).save(destination.parent/name)
            item["preview"], item["mask"] = None, name
        manifest["candidates"].append(item)
    destination.write_text(json.dumps(manifest), encoding="utf-8")


def text_prompts(prompt):
    if not isinstance(prompt, str) or not prompt.strip() or len(prompt) > 256:
        raise ValueError("Describe 1–8 selections in at most 256 characters")
    prompts = [part.strip() for part in prompt.split(",")]
    if not 1 <= len(prompts) <= 8 or any(not part for part in prompts):
        raise ValueError("Describe 1–8 nonempty comma-separated selections")
    return prompts


def text_coverage(logits):
    import torch
    if logits.ndim == 2:
        logits = logits.unsqueeze(0)
    if logits.ndim != 3 or not torch.isfinite(logits).all():
        raise ValueError("Text model returned invalid coverage")
    coverage = logits.float().sigmoid().amax(dim=0).cpu().numpy()
    if float(coverage.max()) < 0.5:
        raise ValueError("No confidently detected text selection; try a different description or point selection")
    # Preserve continuous coverage while giving refinement clear interior/exterior regions.
    return ((coverage - 0.25) / 0.5).clip(0, 1)


def mask(request):
    import numpy as np
    import torch
    from PIL import Image
    from transformers import SamModel, SamProcessor, AutoImageProcessor, AutoModelForDepthEstimation
    image = load_rgb(request)
    proxy = image.copy()
    proxy.thumbnail((2048, 2048))
    where = device(torch)
    pixels_key = image_key(proxy)
    if request.get("operation") == "mask_instances":
        object_instances(request,image,proxy,pixels_key,where)
        return
    if request["kind"] == "text":
        prompts = text_prompts(request.get("prompt"))
        from transformers import CLIPSegProcessor, CLIPSegForImageSegmentation
        key = model_key(TEXT, where)
        processor, model = cached_model(key, lambda: (pretrained(CLIPSegProcessor, TEXT, use_fast=False), pretrained(CLIPSegForImageSegmentation, TEXT, use_safetensors=True).to(where).eval()))
        with torch.inference_mode():
            image_inputs = processor.image_processor(images=[proxy] * len(prompts), return_tensors="pt")
            text_inputs = processor.tokenizer(prompts, padding=True, truncation=True, return_tensors="pt")
            inputs = {key: value.to(where) for key, value in {**image_inputs, **text_inputs}.items()}
            outputs = model(**inputs)
            coarse = text_coverage(outputs.logits)
        del outputs, inputs
    elif request["kind"] in SEMANTIC_SUBJECTS:
        from transformers import MaskFormerImageProcessor, MaskFormerForInstanceSegmentation
        key = model_key(SEMANTIC, where)
        try:
            processor, model = cached_model(key, lambda: (pretrained(MaskFormerImageProcessor, SEMANTIC), pretrained(MaskFormerForInstanceSegmentation, SEMANTIC, use_safetensors=True).to(where).eval()))
        except OSError as error:
            raise ValueError("Element selection models are unavailable. Open Tools → AI runtime and check/install the optional semantic and edge models") from error
        wanted = SEMANTIC_SUBJECTS[request["kind"]][0]
        class_index = semantic_class_index(model.config.id2label, request["kind"])
        with torch.inference_mode():
            def predict_semantic():
                output = model(**processor(images=proxy, return_tensors="pt").to(where))
                return output.class_queries_logits, output.masks_queries_logits
            classes, masks = _PRODUCTS.get((pixels_key, key, "semantic"), predict_semantic)
            from types import SimpleNamespace
            outputs = SimpleNamespace(class_queries_logits=classes, masks_queries_logits=masks)
            coarse = semantic_alpha(outputs, class_index)[0].cpu().numpy()
        if float(coarse.max()) < 0.5:
            raise ValueError(f"No confidently detected {wanted}; use a point selection or painted mask")
        del outputs
    elif request["kind"] == "depth":
        key = model_key(DEPTH, where)
        processor, model = cached_model(key, lambda: (pretrained(AutoImageProcessor, DEPTH, use_fast=False), pretrained(AutoModelForDepthEstimation, DEPTH, use_safetensors=True).to(where).eval()))
        with torch.inference_mode():
            depth = _PRODUCTS.get((pixels_key, key, "depth"), lambda: model(**processor(images=proxy, return_tensors="pt").to(where)).predicted_depth[0].float().cpu().numpy())
        lo, hi = np.percentile(depth, [1, 99])
        depth = np.clip((depth-lo) / max(float(hi-lo), 1e-6), 0, 1)
        start, end = float(request.get("near", 0.5)), float(request.get("far", 1.0))
        soft = 0.06
        coarse = np.clip((depth-start+soft)/soft, 0, 1) * np.clip((end+soft-depth)/soft, 0, 1)
    elif request["kind"] in ("object", "sky"):
        points = request.get("points", [[0.5, 0.5]])
        labels = request.get("labels", [1] * len(points))
        if not 1 <= len(points) <= 32 or len(labels) != len(points) or not any(label == 1 for label in labels) or any(label not in (0,1) for label in labels) or any(len(p)!=2 or any(not np.isfinite(v) or not 0 <= v <= 1 for v in p) for p in points):
            raise ValueError("Use 1–32 finite normalized prompts, including a foreground point, with matching include/exclude labels")
        key = model_key(SAM, where)
        processor, model = cached_model(key, lambda: (pretrained(SamProcessor, SAM), pretrained(SamModel, SAM, use_safetensors=True).to(where).eval()))
        inputs = processor(proxy, input_points=[[[p[0]*proxy.width, p[1]*proxy.height] for p in points]], input_labels=[labels], return_tensors="pt")
        # SamProcessor emits float64 point coordinates; Metal accepts float32.
        inputs = {k: v.to(device=where, dtype=torch.float32 if v.is_floating_point() else v.dtype) for k, v in inputs.items()}
        with torch.inference_mode():
            embeddings = _PRODUCTS.get((pixels_key, key, "sam"), lambda: model.get_image_embeddings(inputs["pixel_values"]))
            outputs = model(image_embeddings=embeddings, **{k: v for k, v in inputs.items() if k != "pixel_values"})
        # Post-process at a bounded proxy size, then align edges against the
        # original RGB. Preserve logits through resizing, never binarize.
        shapes = inputs["original_sizes"].cpu()
        resized = inputs["reshaped_input_sizes"].cpu()
        masks = processor.image_processor.post_process_masks(outputs.pred_masks.cpu(), shapes, resized, binarize=False)[0]
        best = int(outputs.iou_scores[0, 0].argmax())
        coarse = masks[0, best].sigmoid().numpy()
        del outputs, inputs
    else:
        raise ValueError("Unknown mask kind")
    # Drop request intermediates before full-resolution refinement. The bounded
    # weight/product caches retain only reusable models and image inference.
    del model
    if where == "mps": torch.mps.empty_cache()
    elif where == "cuda": torch.cuda.empty_cache()
    rgb = np.asarray(image)
    alpha = refine_alpha(rgb, coarse)
    if request["kind"] != "depth":
        alpha = matte_boundaries(rgb, alpha, where)
    Image.fromarray(quantize_mask(alpha, image.size)).save(request["output"])


def tile_regions(height, width, tile, halo):
    for y in range(0, height, tile):
        for x in range(0, width, tile):
            hh, ww = min(tile, height-y), min(tile, width-x)
            yield y, x, hh, ww, max(0, y-halo), max(0, x-halo), min(height, y+hh+halo), min(width, x+ww+halo)


def region_batches(regions, batch_size):
    """Stream at most four same-sized tiles; never pad edges to batch interiors."""
    batch_size = max(1, min(4, int(batch_size)))
    pending, shape = [], None
    for region in regions:
        _, _, _, _, y0, x0, y1, x1 = region
        current = (y1-y0, x1-x0)
        if pending and (current != shape or len(pending) == batch_size):
            yield pending
            pending = []
        pending.append(region)
        shape = current
    if pending:
        yield pending


def tile_batch_size(where):
    # The local benchmark did not establish a useful default speedup. Keep
    # one tile unless explicitly profiling 2–4; other backends remain untested.
    return setting_int("SHOPPHOTO_ENHANCE_TILE_BATCH", 1, 1, 4)


def matte_boundaries(rgb, coarse, where, batch_size=None):
    """Learn continuous edge alpha from original pixels and a conservative trimap.

    Confident interiors remain exact; the learned head only operates on
    uncertain boundary tiles. This is not a guarantee on transparent objects.
    """
    import cv2
    import numpy as np
    import torch
    from transformers import VitMatteImageProcessor, VitMatteForImageMatting
    processor, model = cached_model(model_key(MATTE, where), lambda: (pretrained(VitMatteImageProcessor, MATTE), pretrained(VitMatteForImageMatting, MATTE, use_safetensors=True).to(where).eval()))
    foreground = (coarse >= 0.5).astype(np.uint8)
    kernel = np.ones((33, 33), np.uint8)
    inside = cv2.erode(foreground, kernel)
    outside = cv2.dilate(foreground, kernel)
    trimap = np.where(inside == 1, 255, np.where(outside == 0, 0, 128)).astype(np.uint8)
    trimap[(coarse > 0.01) & (coarse < 0.99)] = 128
    h, w = coarse.shape
    result = np.where(trimap == 255, 1.0, 0.0).astype(np.float32)
    regions = (region for region in tile_regions(h, w, 512, 64) if (trimap[region[0]:region[0]+region[2], region[1]:region[1]+region[3]] == 128).any())
    for batch in region_batches(regions, tile_batch_size(where) if batch_size is None else batch_size):
        images = [rgb[y0:y1, x0:x1] for _, _, _, _, y0, x0, y1, x1 in batch]
        trimaps = [trimap[y0:y1, x0:x1] for _, _, _, _, y0, x0, y1, x1 in batch]
        inputs = processor(images=images, trimaps=trimaps, return_tensors="pt").to(where)
        with torch.inference_mode():
            alphas = model(**inputs).alphas[:, 0].float().cpu().numpy()
        for alpha, (y, x, hh, ww, y0, x0, _, _) in zip(alphas, batch):
            unknown = trimap[y:y+hh, x:x+ww] == 128
            center = alpha[y-y0:y-y0+hh, x-x0:x-x0+ww]
            region = result[y:y+hh, x:x+ww]
            region[unknown] = np.clip(center[unknown], 0, 1)
    return result


def analyze_faces(request):
    """Optional preview-only frontal face/eye detail assistance; never blink scoring."""
    import cv2
    import numpy as np
    image = load_rgb(request)
    image.thumbnail((1024, 1024))
    gray = cv2.cvtColor(np.asarray(image), cv2.COLOR_RGB2GRAY)
    def cascade(name):
        result = cv2.CascadeClassifier(str(Path(cv2.data.haarcascades)/name))
        if result.empty():
            raise ValueError("OpenCV face/eye detector data is missing")
        return result
    if "face" not in _DETECTORS:
        _DETECTORS["face"] = cascade("haarcascade_frontalface_default.xml")
    if "eye" not in _DETECTORS:
        _DETECTORS["eye"] = cascade("haarcascade_eye_tree_eyeglasses.xml")
    face, eye = _DETECTORS["face"], _DETECTORS["eye"]
    faces = face.detectMultiScale(gray, scaleFactor=1.1, minNeighbors=5, minSize=(40,40))
    faces = sorted(faces, key=lambda p:int(p[2])*int(p[3]), reverse=True)[:32]
    details = []
    for x,y,w,h in faces:
        region = gray[y:y+int(h*0.65), x:x+w]
        found = eye.detectMultiScale(region, scaleFactor=1.1, minNeighbors=5, minSize=(8,8), maxSize=(max(9,w//2),max(9,h//3)))
        accepted = []
        for ex,ey,ew,eh in sorted(found,key=lambda p:int(p[2])*int(p[3]),reverse=True):
            if any(abs(ex+ew/2-ax-aw/2) < (ew+aw)*0.35 for ax,_,aw,_ in accepted):
                continue
            accepted.append((ex,ey,ew,eh))
            patch = region[ey:ey+eh,ex:ex+ew].astype(np.float32)/255
            energy = float(cv2.Laplacian(patch,cv2.CV_32F).var())
            details.append(float(np.clip(np.log1p(energy/max(float(patch.var()),0.001))*35,0,100)))
            if len(accepted)>=2:
                break
    result = {"faces":len(faces),"eye_regions":len(details),"weakest_eye_detail":min(details) if details else None}
    Path(request["output"]).write_text(json.dumps(result),encoding="utf-8")


def tiled_sr(rgb, model, scale, tile=128, halo=32, batch_size=1):
    import numpy as np
    import torch
    h, w, _ = rgb.shape
    # Output lives in host RAM; only a bounded group of tiles uses the GPU.
    result = np.empty((h*scale, w*scale, 3), dtype=np.float32)
    for batch in region_batches(tile_regions(h, w, tile, halo), batch_size):
        tensors = [torch.from_numpy(rgb[y0:y1, x0:x1].copy()).permute(2, 0, 1) for _, _, _, _, y0, x0, y1, x1 in batch]
        tensor = torch.stack(tensors).to(device=model.device, dtype=model.dtype)
        with torch.inference_mode():
            predictions = model(tensor).float().cpu().permute(0, 2, 3, 1).numpy()
        for predicted, (y, x, hh, ww, y0, x0, _, _) in zip(predictions, batch):
            result[y*scale:(y+hh)*scale, x*scale:(x+ww)*scale] = predicted[(y-y0)*scale:(y-y0+hh)*scale, (x-x0)*scale:(x-x0+ww)*scale]
    return result


def upscale(request):
    import cv2
    import numpy as np
    import torch
    from spandrel import ModelLoader, ImageModelDescriptor
    if Path(request["model"]).suffix.lower() not in (".pth", ".safetensors"):
        raise ValueError("Only .pth state dictionaries or .safetensors checkpoints are accepted")
    checkpoint = Path(request["model"]).resolve()
    stamp = checkpoint.stat()
    where = device(torch)
    model = cached_model((str(checkpoint), stamp.st_size, stamp.st_mtime_ns, where), lambda: ModelLoader().load_from_file(checkpoint).to(where).eval().float())
    if model.architecture.id not in ("DRCT", "HAT", "SwinIR"):
        raise ValueError("Choose a DRCT, HAT or SwinIR super-resolution checkpoint")
    if not isinstance(model, ImageModelDescriptor) or model.input_channels != 3 or model.output_channels != 3:
        raise ValueError("Choose an RGB image super-resolution checkpoint (DRCT, HAT or SwinIR)")
    if model.scale not in (2, 4):
        raise ValueError("Choose a native 2× or 4× super-resolution model")
    model = model.to(device(torch)).eval()
    # Float32 is the reference path. Device-specific FP16 requires parity tests.
    model = model.float()
    # The bundled optional SwinIR path is parity-tested for small batches.
    # Arbitrary HAT/DRCT checkpoints keep their single-image contract.
    batch_size = tile_batch_size(where) if model.architecture.id == "SwinIR" else 1
    pixels = cv2.imread(request["input"], cv2.IMREAD_UNCHANGED)
    if pixels is None:
        raise ValueError("Could not read upscale input")
    rgb = pixels[:, :, :3][:, :, ::-1].astype(np.float32) / (65535 if pixels.dtype == np.uint16 else 255)
    scale = model.scale
    if rgb.shape[0]*rgb.shape[1]*scale*scale > 150_000_000:
        raise ValueError("Upscale exceeds 150 MP; crop more tightly or select a 2× model")
    if request.get("hdr", False):
        # ST 2084 -> absolute scene light. Master luminance/chroma stay intact.
        m1, m2, c1, c2, c3 = 2610/16384, 2523/32, 3424/4096, 2413/128, 2392/128
        p = np.maximum(rgb, 0)**(1/m2)
        linear = 10000*(np.maximum(p-c1, 0)/np.maximum(c2-c3*p, 1e-6))**(1/m1)
        luma = np.sum(linear*np.array([0.2627, 0.6780, 0.0593], np.float32), axis=2, keepdims=True)
        # Fixed compressive guide: chroma remains in the master branch.
        # Rec.2020 linear master -> sRGB primaries for the SDR-trained model.
        to_srgb = np.array([[1.660491, -0.587641, -0.072850],
                            [-0.124550, 1.132900, -0.008349],
                            [-0.018151, -0.100579, 1.118730]], np.float32)
        guide = np.clip((linear/(luma+203)) @ to_srgb.T, 0, 1)
        guide = np.where(guide <= 0.0031308, 12.92*guide, 1.055*guide**(1/2.4)-0.055)
        ai = tiled_sr(guide, model, scale, batch_size=batch_size)
        size = (rgb.shape[1]*scale, rgb.shape[0]*scale)
        base = cv2.resize(linear, size, interpolation=cv2.INTER_LINEAR)
        ordinary = cv2.resize(guide, size, interpolation=cv2.INTER_LINEAR)
        residual = np.sum((ai-ordinary)*np.array([0.2126, 0.7152, 0.0722], np.float32), axis=2, keepdims=True)
        # Bounded exposure modulation, fading around black and specular peaks.
        y = np.sum(base*np.array([0.2627, 0.6780, 0.0593], np.float32), axis=2, keepdims=True)
        confidence = np.clip(y/4, 0, 1) * np.clip((4000-y)/1000, 0, 1)
        enhanced = base * 2**(np.clip(residual*2, -0.20, 0.20)*confidence)
        p = (np.maximum(enhanced, 0)/10000)**m1
        output = ((c1+c2*p)/(1+c3*p))**m2
    else:
        output = tiled_sr(rgb, model, scale, batch_size=batch_size)
    output = np.round(np.clip(output, 0, 1)*65535).astype(np.uint16)
    if not cv2.imwrite(request["output"], output[:, :, ::-1]):
        raise ValueError("Could not write enhanced image")
    Path(request["output"] + ".json").write_text(json.dumps({"scale": scale, "architecture": model.architecture.name}), encoding="utf-8")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("request", nargs="?")
    parser.add_argument("--download-masks", action="store_true")
    parser.add_argument("--download-semantic", action="store_true", help="Install optional CC-BY-NC MaskFormer people/sky model")
    parser.add_argument("--download-text", action="store_true", help="Install optional CLIPSeg text selection and ViTMatte edge models")
    parser.add_argument("--status", action="store_true")
    parser.add_argument("--serve", action="store_true")
    args = parser.parse_args()
    if args.status:
        print(json.dumps(status()))
        return
    if args.download_text:
        from transformers import CLIPSegProcessor, CLIPSegForImageSegmentation, VitMatteImageProcessor, VitMatteForImageMatting
        pretrained(CLIPSegProcessor, TEXT, download=True, use_fast=False)
        pretrained(CLIPSegForImageSegmentation, TEXT, download=True, use_safetensors=True)
        pretrained(VitMatteImageProcessor, MATTE, download=True)
        pretrained(VitMatteForImageMatting, MATTE, download=True, use_safetensors=True)
        return
    if args.download_semantic:
        from transformers import MaskFormerImageProcessor, MaskFormerForInstanceSegmentation
        pretrained(MaskFormerImageProcessor, SEMANTIC, download=True)
        pretrained(MaskFormerForInstanceSegmentation, SEMANTIC, download=True, use_safetensors=True)
        return
    if args.download_masks:
        load_detector(download=True)
        from transformers import SamModel, SamProcessor, AutoImageProcessor, AutoModelForDepthEstimation, VitMatteImageProcessor, VitMatteForImageMatting
        pretrained(SamProcessor, SAM, download=True)
        pretrained(SamModel, SAM, download=True, use_safetensors=True)
        pretrained(AutoImageProcessor, DEPTH, download=True, use_fast=False)
        pretrained(AutoModelForDepthEstimation, DEPTH, download=True, use_safetensors=True)
        pretrained(VitMatteImageProcessor, MATTE, download=True)
        pretrained(VitMatteForImageMatting, MATTE, download=True, use_safetensors=True)
        return
    if args.serve:
        for line in sys.stdin:
            # A malformed envelope has no reliable response destination. Log
            # it and keep serving later requests instead of killing the worker.
            try:
                request = json.loads(line)
                response = Path(request.pop("_response"))
            except (ValueError, TypeError, KeyError, AttributeError) as error:
                print(f"Invalid enhancement request: {error}", file=sys.stderr)
                continue
            try:
                dispatch(request)
                result = {"ok": True}
            except Exception as error:
                import traceback
                traceback.print_exc()
                result = {"ok": False, "error": str(error)}
            try:
                temporary = response.with_suffix(".pending")
                temporary.write_text(json.dumps(result), encoding="utf-8")
                temporary.replace(response)
            except (OSError, ValueError) as error:
                print(f"Could not write enhancement response: {error}", file=sys.stderr)
        return
    if not args.request:
        parser.error("a request file, --serve, --status or --download-masks is required")
    dispatch(json.loads(Path(args.request).read_text(encoding="utf-8")))


def dispatch(request):
    if request["operation"] == "analyze_faces":
        analyze_faces(request)
    elif request["operation"] in ("mask", "mask_instances"):
        mask(request)
    elif request["operation"] == "upscale":
        upscale(request)
    else:
        raise ValueError("Unknown enhancement operation")


if __name__ == "__main__":
    main()
