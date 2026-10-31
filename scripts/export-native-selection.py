#!/usr/bin/env python3
"""Offline Core ML export of SSD proposals and cached SlimSAM encoder/decoders.

Only trusted local weights and pinned cached safetensors are read. Python is an
export dependency, not an application runtime dependency for these bundles.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile
import time
from native_model_notices import attach_native_notices

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("task",choices=("detector","sam-encoder","sam-box","sam-points"))
    parser.add_argument("--output",required=True,type=Path)
    parser.add_argument("--checkpoint",type=Path,default=Path.home()/".cache/torch/hub/checkpoints/ssdlite320_mobilenet_v3_large_coco-a79551df.pth")
    parser.add_argument("--image",type=Path,default=Path(__file__).resolve().parents[1]/"docs/advanced-ai-features/ai-mask-scene.png")
    args=parser.parse_args()
    if args.output.exists():parser.error("output must be new")
    import coremltools as ct
    import numpy as np
    import torch
    from PIL import Image
    import enhance
    torch.set_num_threads(6);torch.manual_seed(173)
    image=Image.open(args.image).convert("RGB")
    meta={"version":1,"model":"model.mlmodelc","dtype":"float32","precision":"float32","minimum_macos":"15.0","toolchain":{"torch":torch.__version__,"coremltools":ct.__version__}}
    if args.task=="detector":
        from torchvision.models.detection import ssdlite320_mobilenet_v3_large
        from torchvision.models.detection.image_list import ImageList
        source=ssdlite320_mobilenet_v3_large(weights=None,weights_backbone=None).eval()
        source.load_state_dict(torch.load(args.checkpoint,map_location="cpu",weights_only=True))
        class Detector(torch.nn.Module):
            def __init__(self,wrapped):
                super().__init__();self.wrapped=wrapped
                with torch.inference_mode():
                    sample=torch.zeros(1,3,320,320)
                    features=list(wrapped.backbone(sample).values())
                    anchors=wrapped.anchor_generator(ImageList(sample,[(320,320)]),features)[0]
                self.register_buffer("anchors",anchors)
            def forward(self,pixels):
                features=list(self.wrapped.backbone(pixels).values())
                head=self.wrapped.head(features)
                boxes=self.wrapped.box_coder.decode_single(head["bbox_regression"][0],self.anchors)
                return boxes.unsqueeze(0),head["cls_logits"]
        model=Detector(source).eval()
        sample=torch.from_numpy(np.asarray(image.resize((320,320)),np.float32).transpose(2,0,1).copy()/127.5-1).unsqueeze(0)
        inputs={"pixels":sample};names=["boxes","class_logits"]
        meta.update(id="ssdlite-coco",task="detector",classes={"people":1,"cars":3,"motorcycles":4},license="BSD-3-Clause (code); pretrained dataset terms apply",source="https://download.pytorch.org/models/ssdlite320_mobilenet_v3_large_coco-a79551df.pth",revision=digest(args.checkpoint),checkpoint_sha256=digest(args.checkpoint),preprocessing="RGB resize directly320x320; (rgb/255-.5)/.5. Outputdecodedboxes in320pixelspace; hostclipcoordinates,softmax91classes,NMS.55,score.45.",nms_threshold=.55,score_threshold=.45)
    else:
        from transformers import SamModel,SamProcessor
        source=enhance.pretrained(SamModel,enhance.SAM,use_safetensors=True).eval()
        processor=enhance.pretrained(SamProcessor,enhance.SAM)
        pixels=processor(image,return_tensors="pt")["pixel_values"]
        with torch.inference_mode():embeddings=source.get_image_embeddings(pixels)
        if args.task=="sam-encoder":
            class Encoder(torch.nn.Module):
                def __init__(self,wrapped):super().__init__();self.wrapped=wrapped
                def forward(self,pixels):return self.wrapped.get_image_embeddings(pixels)
            model=Encoder(source).eval();inputs={"pixels":pixels};names=["embeddings"]
            preprocess="RGB resize longest1024 with roundedsize floor(old*scale+.5), normalize(rgb/255-mean)/std ImageNet, zero padright/bottom normalizedspace."
        elif args.task=="sam-box":
            class Box(torch.nn.Module):
                def __init__(self,wrapped):super().__init__();self.wrapped=wrapped
                def forward(self,embeddings,boxes):
                    result=self.wrapped(image_embeddings=embeddings,input_boxes=boxes,multimask_output=True)
                    return result.pred_masks,result.iou_scores
            model=Box(source).eval();inputs={"embeddings":embeddings,"boxes":torch.tensor([[[256.,128.,768.,896.]]])};names=["mask_logits","iou_scores"]
            preprocess="boxes [x0,y0,x1,y1] in aspectresized1024inputpixelspace (beforezero pad); outputlogitsupsample1024,croptoresizedextent,resizeoriginal; choosehighestIoUof3,sigmoid."
        else:
            class Points(torch.nn.Module):
                def __init__(self,wrapped):super().__init__();self.wrapped=wrapped
                def forward(self,embeddings,points,labels):
                    result=self.wrapped(image_embeddings=embeddings,input_points=points,input_labels=labels.to(torch.int64),multimask_output=True)
                    return result.pred_masks,result.iou_scores
            model=Points(source).eval()
            points=torch.tensor([[[[512.,512.],[256.,256.]]]])
            labels=torch.tensor([[[1.,0.]]])
            inputs={"embeddings":embeddings,"points":points,"labels":labels};names=["mask_logits","iou_scores"]
            preprocess="1-32 realpoints in aspectresized1024pixelspace; labels float0exclude/1include. No artificialpaddingtokens. Flexiblepointcountaxis2."
        meta.update(id="slimsam-"+args.task.removeprefix("sam-"),task=args.task.replace("-","_"),license="Apache-2.0",source="https://huggingface.co/"+enhance.SAM,revision=enhance.REVISIONS[enhance.SAM],preprocessing=preprocess)
    with torch.inference_mode():
        expected=model(*inputs.values());expected=expected if isinstance(expected,tuple) else (expected,)
        expected=[item.numpy() for item in expected]
        traced=torch.jit.trace(model,tuple(inputs.values()),strict=False,check_trace=False)
    args.output.parent.mkdir(parents=True,exist_ok=True)
    staging=Path(tempfile.mkdtemp(prefix=".selection-export-",dir=args.output.parent));started=time.perf_counter()
    try:
        converted=ct.convert(traced,inputs=[ct.TensorType(name=name,shape=tuple(ct.RangeDim(lower_bound=1,upper_bound=32,default=int(dimension)) if args.task == "sam-points" and name in ("points","labels") and axis == 2 else int(dimension) for axis,dimension in enumerate(value.shape)),dtype=np.float32) for name,value in inputs.items()],outputs=[ct.TensorType(name=name,dtype=np.float32) for name in names],convert_to="mlprogram",minimum_deployment_target=ct.target.macOS15,compute_precision=ct.precision.FLOAT32,compute_units=ct.ComputeUnit.ALL)
        report=[]
        retained_arrays=[]
        cases=[inputs]
        if args.task == "sam-points":
            for count in (1,3,32):
                real_points=torch.linspace(128,896,count*2).reshape(1,1,count,2)
                real_labels=torch.ones((1,1,count));real_labels[...,1::2]=0
                cases.append({"embeddings":embeddings,"points":real_points,"labels":real_labels})
        for case in cases:
            arrays={name:value.numpy().copy() for name,value in case.items()}
            retained_arrays.append(arrays)
            actual=converted.predict(arrays)
            with torch.inference_mode():
                reference_values=model(*case.values())
                reference_values=reference_values if isinstance(reference_values,tuple) else (reference_values,)
            for name,reference_tensor in zip(names,reference_values):
                reference=reference_tensor.numpy()
                result=actual[name]
                if result.shape!=reference.shape or not np.isfinite(result).all():raise ValueError("invalid native pixels or dimensions")
                error=np.abs(result-reference)
                np.testing.assert_allclose(result,reference,rtol=.003,atol=.003)
                report.append({"name":name,"point_count":int(case["points"].shape[2]) if args.task == "sam-points" else None,"max_abs":float(error.max()),"mean_abs":float(error.mean())})
        package=staging/"source.mlpackage";converted.save(str(package))
        compiled=Path(ct.models.utils.compile_model(str(package)));shutil.copytree(compiled,staging/"model.mlmodelc");shutil.rmtree(package)
        meta["inputs"]=[{"name":name,"shape":list(value.shape), **({"flexible_axes":{"2":[1,32]}} if args.task == "sam-points" and name in ("points","labels") else {})} for name,value in inputs.items()]
        meta["outputs"]=[{"name":name,"shape":list(value.shape)} for name,value in zip(names,expected)]
        meta["validation"]={"cpu_reference_parity":report,"coreml_compute_units":"ALL","conversion_seconds":time.perf_counter()-started,"image":args.image.name,"scope":"actualimage graphparity; not qualitybenchmark"}
        attach_native_notices(staging, meta)
        meta["files"]={str(path.relative_to(staging)):digest(path) for path in sorted(staging.rglob("*")) if path.is_file()}
        (staging/"manifest.json").write_text(json.dumps(meta,indent=2)+"\n");staging.rename(args.output);print(json.dumps(meta,indent=2))
    finally:
        if staging.exists():shutil.rmtree(staging)


if __name__=="__main__":main()
