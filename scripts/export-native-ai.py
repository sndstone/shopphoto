#!/usr/bin/env python3
"""Export pinned enhancement checkpoints to validated native Core ML bundles.

Development/setup tool only. The installed application never imports Python.
No downloads occur: semantic/matte weights must already be in the HF cache, and
SwinIR requires an explicit trusted weights-only checkpoint. Outputs are staged,
compared with CPU reference outputs, then atomically renamed into a new bundle.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile
import time
import types

from native_model_notices import attach_native_notices

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("task", choices=("semantic", "oneformer", "matte", "upscale"))
    parser.add_argument("--checkpoint", type=Path)
    parser.add_argument("--output", type=Path, required=True, help="new native bundle directory")
    parser.add_argument("--package", type=Path, help="optional copy of source .mlpackage outside runtime bundle")
    parser.add_argument("--image", type=Path, default=Path(__file__).resolve().parents[1]/"docs/advanced-ai-features/ai-mask-scene.png")
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output already exists; remove or move it explicitly before rebuilding")
    if args.package and (args.package.exists() or args.package.suffix != ".mlpackage"):
        parser.error("--package must be a new .mlpackage path")
    if args.task == "upscale" and not args.checkpoint:
        parser.error("upscale requires --checkpoint")
    import coremltools as ct
    import numpy as np
    import torch
    from PIL import Image
    import enhance
    torch.set_num_threads(6)
    torch.manual_seed(173)
    rgb = Image.open(args.image).convert("RGB")
    rgb.thumbnail((512, 512), Image.Resampling.BILINEAR)
    normalized = np.zeros((512, 512, 3), np.float32)
    normalized[:rgb.height, :rgb.width] = (np.asarray(rgb, dtype=np.float32)/255 - np.array([.485,.456,.406], np.float32))/np.array([.229,.224,.225], np.float32)
    metadata = {"version": 1, "task": args.task, "model": "model.mlmodelc", "dtype": "float32", "minimum_macos": "15.0", "precision": "float32", "toolchain": {"torch": torch.__version__, "coremltools": ct.__version__}}
    if args.task == "upscale":
        from spandrel import ModelLoader
        descriptor = ModelLoader().load_from_file(args.checkpoint).eval().float()
        if descriptor.architecture.id != "SwinIR" or descriptor.scale not in (2,4) or descriptor.input_channels != 3 or descriptor.output_channels != 3:
            parser.error("checkpoint must be RGB SwinIR native2x/4x")
        model = descriptor.model
        scale = descriptor.scale
        pixels = torch.from_numpy(np.asarray(rgb.resize((192,192)), np.float32).transpose(2,0,1).copy()/255).unsqueeze(0)
        names = ["rgb"]
        metadata.update(id=f"swinir-{scale}x", scale=scale, tile=192, halo=32, checkpoint_sha256=sha256(args.checkpoint), license="Apache-2.0", source="https://github.com/JingyunLiang/SwinIR", revision=sha256(args.checkpoint), preprocessing="RGB sRGB in [0,1]; model mean remains inside graph; fixed192x192 tile includes32px halo")
    elif args.task == "oneformer":
        from transformers import OneFormerForUniversalSegmentation, OneFormerProcessor
        from coremltools.converters.mil.frontend.torch.torch_op_registry import register_torch_op
        from coremltools.converters.mil.frontend.torch.ops import _get_inputs
        from coremltools.converters.mil.mil import Builder as mb
        @register_torch_op
        def prod(context,node):
            values=_get_inputs(context,node)
            x,axis=values[0],values[1]
            if x.val is None or axis.val is None:
                raise ValueError("Only fixedshape constant positional products are exported")
            keep=bool(values[2].val) if len(values)>2 and values[2] is not None else False
            result=np.prod(x.val,axis=int(axis.val),keepdims=keep).astype(np.int32)
            context.add(mb.const(val=result,name=node.name))
        revision = "05f2812b1eccf9909b3897777450f8d68148cafc"
        model_id = "shi-labs/oneformer_ade20k_swin_tiny"
        source = OneFormerForUniversalSegmentation.from_pretrained(model_id,revision=revision,local_files_only=True,weights_only=True).eval()
        processor = OneFormerProcessor.from_pretrained(model_id,revision=revision,local_files_only=True)
        task = processor(images=rgb,task_inputs=["semantic"],return_tensors="pt")["task_inputs"]
        from transformers.models.oneformer.modeling_oneformer import OneFormerPixelDecoderEncoderMultiscaleDeformableAttention
        def rank5_attention(self,hidden_states,attention_mask=None,encoder_hidden_states=None,encoder_attention_mask=None,position_embeddings=None,reference_points=None,spatial_shapes=None,level_start_index=None,output_attentions=False):
            # Algebraically identical deformable attention; fuse level/point
            # axes only when necessary to keep every intermediate rank <=5.
            if position_embeddings is not None:hidden_states=hidden_states+position_embeddings
            batch,queries,_=hidden_states.shape
            value=self.value_proj(encoder_hidden_states)
            if attention_mask is not None:value=value.masked_fill(attention_mask[...,None],0.)
            head_dim=self.d_model//self.n_heads
            value=value.reshape(batch,-1,self.n_heads,head_dim)
            offsets=self.sampling_offsets(hidden_states).reshape(batch,queries,self.n_heads,self.n_levels,self.n_points*2)
            weights=self.attention_weights(hidden_states).reshape(batch,queries,self.n_heads,self.n_levels*self.n_points).softmax(-1)
            samples=[];level_weights=[];start=0
            for level in range(self.n_levels):
                height,width=int(spatial_shapes[level,0]),int(spatial_shapes[level,1])
                level_value=value[:,start:start+height*width].flatten(2).transpose(1,2).reshape(batch*self.n_heads,head_dim,height,width)
                level_offsets=offsets[:,:,:,level].reshape(batch,queries,self.n_heads,self.n_points,2)
                normalizer=level_offsets.new_tensor([width,height])
                locations=reference_points[:,:,level,:][:,:,None,None,:]+level_offsets/normalizer
                grid=(2*locations-1).transpose(1,2).reshape(batch*self.n_heads,queries,self.n_points,2)
                samples.append(torch.nn.functional.grid_sample(level_value,grid,mode="bilinear",padding_mode="zeros",align_corners=False))
                level_weights.append(weights[:,:,:,level*self.n_points:(level+1)*self.n_points].transpose(1,2).reshape(batch*self.n_heads,1,queries,self.n_points))
                start+=height*width
            sampled=torch.cat(samples,dim=-1);attention=torch.cat(level_weights,dim=-1)
            result=(sampled*attention).sum(-1).reshape(batch,self.n_heads*head_dim,queries).transpose(1,2).contiguous()
            return self.output_proj(result),weights.reshape(batch,queries,self.n_heads,self.n_levels,self.n_points)
        reference_pixels=torch.from_numpy(normalized.transpose(2,0,1).copy()).unsqueeze(0)
        with torch.inference_mode():
            unmodified=source(pixel_values=reference_pixels,task_inputs=task)
            unmodified=(unmodified.class_queries_logits.clone(),unmodified.masks_queries_logits.clone())
        for module in source.modules():
            if isinstance(module,OneFormerPixelDecoderEncoderMultiscaleDeformableAttention):
                module.forward=types.MethodType(rank5_attention,module)
        with torch.inference_mode():
            altered=source(pixel_values=reference_pixels,task_inputs=task)
            torch.testing.assert_close(altered.class_queries_logits,unmodified[0],rtol=.0001,atol=.0001)
            torch.testing.assert_close(altered.masks_queries_logits,unmodified[1],rtol=.0001,atol=.0001)
        class OneFormer(torch.nn.Module):
            def __init__(self,wrapped):
                super().__init__();self.wrapped=wrapped;self.register_buffer("task",task)
            def forward(self,pixels):
                result=self.wrapped(pixel_values=pixels,task_inputs=self.task)
                return result.class_queries_logits,result.masks_queries_logits
        model=OneFormer(source)
        pixels=torch.from_numpy(normalized.transpose(2,0,1).copy()).unsqueeze(0)
        names=["class_logits","mask_logits"]
        metadata.update(id="oneformer-ade",task="semantic",classes={"sky":2,"people":12,"cars":20,"motorcycles":116},license="MIT",source="https://huggingface.co/"+model_id,revision=revision,preprocessing="RGB ImageNet normalized; resize longest512 preservingaspect; right/bottom normalizedzero pad; semantic task tokens fixedinsidegraph; class150no-object")
    elif args.task == "semantic":
        from transformers import MaskFormerForInstanceSegmentation
        source = enhance.pretrained(MaskFormerForInstanceSegmentation, enhance.SEMANTIC, use_safetensors=True).eval()
        class Semantic(torch.nn.Module):
            def __init__(self, wrapped):
                super().__init__(); self.wrapped = wrapped
            def forward(self, pixels):
                result = self.wrapped(pixel_values=pixels)
                return result.class_queries_logits, result.masks_queries_logits
        model = Semantic(source)
        pixels = torch.from_numpy(normalized.transpose(2,0,1).copy()).unsqueeze(0)
        names = ["class_logits", "mask_logits"]
        metadata.update(id="maskformer-ade", classes={"sky":2,"people":12,"cars":20,"motorcycles":116}, license="CC-BY-NC-4.0", source="https://huggingface.co/"+enhance.SEMANTIC, revision=enhance.REVISIONS[enhance.SEMANTIC], preprocessing="RGB ImageNet mean[.485,.456,.406] std[.229,.224,.225]; resize longest512 preservingaspect; right/bottom zero padding in normalizedspace; class150 is no-object")
    else:
        from transformers import VitMatteForImageMatting
        from transformers.models.vitdet import modeling_vitdet
        # Fixedshape tracing must inline the original function rather than
        # leave a dynamic TorchScript branch in the CoreML graph.
        modeling_vitdet.get_rel_pos = modeling_vitdet.get_rel_pos.__wrapped__
        source = enhance.pretrained(VitMatteForImageMatting, enhance.MATTE, use_safetensors=True).eval()
        class Matte(torch.nn.Module):
            def __init__(self, wrapped):
                super().__init__(); self.wrapped = wrapped
            def forward(self, pixels):
                return self.wrapped(pixel_values=pixels).alphas
        # Fixedshape positional interpolation is independent of image pixels.
        # Freeze its exact bicubic result rather than replacing it with a
        # lowerquality interpolation merely to satisfy the exporter.
        embedding = source.backbone.embeddings
        with torch.inference_mode():
            position = embedding.get_absolute_positions(embedding.position_embeddings, True, 32, 32)
        embedding.register_buffer("native_positions", position)
        def fixed_positions(self, positions, has_cls, height, width):
            return self.native_positions
        embedding.get_absolute_positions = types.MethodType(fixed_positions, embedding)
        model = Matte(source)
        trimap = np.zeros((512,512), np.float32);trimap[:,160:352] = .5;trimap[:,352:] = 1
        pixels = torch.from_numpy(np.concatenate((normalized.transpose(2,0,1),trimap[None]),axis=0).copy()).unsqueeze(0)
        names = ["alpha"]
        metadata.update(id="vitmatte-small", tile=512, halo=64, license="MIT", source="https://huggingface.co/"+enhance.MATTE, revision=enhance.REVISIONS[enhance.MATTE], preprocessing="packed RGB ImageNet normalized plus trimap0/.5/1; fixed512x512 boundarytile includes64pxhalo")
    model.eval().float()
    with torch.inference_mode():
        expected = model(pixels)
        expected = [item.detach().numpy() for item in (expected if isinstance(expected,tuple) else (expected,))]
        traced = torch.jit.trace(model, pixels, strict=False, check_trace=False)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".native-export-", dir=args.output.parent))
    started = time.perf_counter()
    try:
        converted = ct.convert(traced, inputs=[ct.TensorType(name="pixels",shape=tuple(pixels.shape),dtype=np.float32)],outputs=[ct.TensorType(name=name,dtype=np.float32) for name in names],convert_to="mlprogram",minimum_deployment_target=ct.target.macOS15,compute_precision=ct.precision.FLOAT32,compute_units=ct.ComputeUnit.ALL)
        package = staging/"source.mlpackage"
        converted.save(str(package))
        reports = []
        # CoreML can retain an execution stream after predict returns. Retain
        # owning NumPy inputs until model teardown (never pass temporary views).
        retained_inputs = []
        # Check two images, including a flat low-frequency input, to expose graph
        # constants that accidentally depended on trace data.
        for test_pixels in (pixels, pixels*.5):
            with torch.inference_mode():
                reference = model(test_pixels)
                reference = reference if isinstance(reference,tuple) else (reference,)
            native_input = test_pixels.numpy().copy()
            retained_inputs.append(native_input)
            actual = converted.predict({"pixels": native_input})
            for name, expected_tensor in zip(names,reference):
                expected_array = expected_tensor.numpy()
                actual_array = actual[name]
                if actual_array.shape != expected_array.shape or not np.isfinite(actual_array).all():
                    raise ValueError(f"invalid native result {name}: actualshape{actual_array.shape} expected{expected_array.shape} finite{np.isfinite(actual_array).sum()}/{actual_array.size}")
                error = np.abs(actual_array-expected_array)
                # Float32 CoreML softmax/convolution reductions need practical
                # tolerance while still rejecting material conversion changes.
                if args.task != "oneformer":
                    np.testing.assert_allclose(actual_array,expected_array,rtol=.002,atol=.002)
                reports.append({"name":name,"max_abs":float(error.max()),"mean_abs":float(error.mean())})
            if args.task == "oneformer":
                # Deformable resampling can change raw logits near zero while
                # preserving posteriors. Gate the actual semantic coverage, not
                # relative error of a logit whose reference is almost zero.
                reference_classes,reference_masks=[item.numpy() for item in reference]
                def probabilities(logits):
                    values=np.exp(logits-logits.max(axis=-1,keepdims=True))
                    return values/values.sum(axis=-1,keepdims=True)
                classes=probabilities(actual["class_logits"])[...,:-1]
                reference_classes=probabilities(reference_classes)[...,:-1]
                masks=1/(1+np.exp(-np.clip(actual["mask_logits"],-80,80)))
                reference_masks=1/(1+np.exp(-np.clip(reference_masks,-80,80)))
                posterior_error=float(np.abs(classes-reference_classes).max())
                if posterior_error>.005:raise ValueError(f"class probability parity failed {posterior_error}")
                for subject,index in metadata["classes"].items():
                    coverage=np.einsum("bq,bqhw->bhw",classes[...,index],masks)/np.maximum(np.einsum("bq,bqhw->bhw",classes.sum(-1),masks),1e-6)
                    reference_coverage=np.einsum("bq,bqhw->bhw",reference_classes[...,index],reference_masks)/np.maximum(np.einsum("bq,bqhw->bhw",reference_classes.sum(-1),reference_masks),1e-6)
                    error=np.abs(coverage-reference_coverage)
                    if error.max()>.01 or error.mean()>.0005:raise ValueError(f"semantic coverage parity failed {subject}: max{error.max()} mean{error.mean()}")
                    reports.append({"name":subject+"_coverage","max_abs":float(error.max()),"mean_abs":float(error.mean()),"max_class_probability_error":posterior_error})
        compiled = Path(ct.models.utils.compile_model(str(package)))
        shutil.copytree(compiled, staging/"model.mlmodelc")
        if args.package:
            args.package.parent.mkdir(parents=True, exist_ok=True)
            shutil.copytree(package,args.package)
        shutil.rmtree(package)
        metadata["inputs"] = [{"name":"pixels","shape":list(pixels.shape)}]
        metadata["outputs"] = [{"name":name,"shape":list(value.shape)} for name,value in zip(names,expected)]
        metadata["validation"] = {"cpu_reference_parity":reports,"coreml_compute_units":"ALL","conversion_seconds":time.perf_counter()-started,"image":args.image.name,"scope":"two actualimage tensors; not segmentation-quality benchmark"}
        attach_native_notices(staging, metadata)
        metadata["files"] = {str(path.relative_to(staging)):sha256(path) for path in sorted(staging.rglob("*")) if path.is_file()}
        (staging/"manifest.json").write_text(json.dumps(metadata,indent=2)+"\n",encoding="utf-8")
        staging.rename(args.output)
        print(json.dumps(metadata,indent=2))
    finally:
        if staging.exists():
            shutil.rmtree(staging)


if __name__ == "__main__":
    main()
