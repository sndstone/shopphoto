"""Numerical regression tests; set SHOPPHOTO_TEST_MASKS=1 for real-model smoke tests."""
import os
from pathlib import Path
import tempfile
import unittest
import subprocess
import sys
import json
import time
from unittest import mock
import numpy as np
import torch
from PIL import Image
import enhance


class Nearest:
    device = "cpu"
    dtype = torch.float32

    def __call__(self, value):
        return torch.nn.functional.interpolate(value, scale_factor=2, mode="nearest")


class EnhancementTests(unittest.TestCase):
    def test_subject_labels_match_whole_aliases_and_reject_ambiguous_classes(self):
        labels = {2: "sky", 12: "person", 20: "car", 48: "skyscraper", 116: "minibike, motorbike"}
        for kind, expected in [("semantic_sky", 2), ("semantic_people", 12), ("semantic_cars", 20), ("semantic_motorcycles", 116)]:
            self.assertEqual(enhance.semantic_class_index(labels, kind), expected)
        with self.assertRaisesRegex(ValueError, "unique car"):
            enhance.semantic_class_index({48: "skyscraper"}, "semantic_cars")
        with self.assertRaisesRegex(ValueError, "unique"):
            enhance.semantic_class_index({0: "motorbike", 1: "motorcycle"}, "semantic_motorcycles")

    def test_instance_boxes_separate_touching_objects_and_exclude_other_classes(self):
        prediction={"labels":torch.tensor([1,1,3,4,1,1]),"scores":torch.tensor([.95,.9,.8,.85,.2,float('nan')]),
                    "boxes":torch.tensor([[0,0,4,4],[4,0,8,4],[8,0,12,4],[0,4,4,8],[0,0,4,4],[0,0,4,4]])}
        people=enhance.detector_candidates(prediction,'semantic_people',(12,8))
        self.assertEqual([item['id'] for item in people],[0,1])
        self.assertEqual([item['id'] for item in enhance.detector_candidates(prediction,'semantic_cars',(12,8))],[2])
        self.assertEqual([item['id'] for item in enhance.detector_candidates(prediction,'semantic_motorcycles',(12,8))],[3])
        self.assertEqual(people[0]['bounds'],[0.,0.,1/3,.5])
        self.assertEqual(people[1]['bounds'],[1/3,0.,1/3,.5])

    def test_instance_selection_rejects_stale_ids_and_only_refines_chosen_object(self):
        def candidates():
            left=np.zeros((4,8),np.float32);left[:,:4]=1
            right=1-left
            return [{"id":index,"confidence":.9,"bounds":[index*.5,0.,.5,1.],"alpha":alpha} for index,alpha in enumerate((left,right))]
        image = Image.new('RGB',(80,40),(20,30,40))
        with tempfile.TemporaryDirectory() as tmp:
            out=Path(tmp)/'instances.json'
            request={'kind':'semantic_people','output':str(out)}
            with mock.patch.object(enhance,'matte_boundaries',side_effect=lambda rgb,alpha,where:alpha) as refine:
                enhance.save_instances(request,image,candidates(),'cpu')
                manifest=json.loads(out.read_text())
                self.assertEqual(len(manifest['candidates']),2)
                self.assertIsNone(manifest['candidates'][0]['mask'])
                self.assertFalse(refine.called,'detection must not allocate full-resolution mattes')
                for ids in ([9],[0,0],[],[True]):
                    with self.assertRaises(ValueError):
                        enhance.save_instances({**request,'selected_ids':ids},image,candidates(),'cpu')
                enhance.save_instances({**request,'selected_ids':[1]},image,candidates(),'cpu')
                selected=json.loads(out.read_text())['candidates']
                self.assertEqual([item['id'] for item in selected],[1])
                self.assertEqual(refine.call_count,1)
                coverage=np.asarray(Image.open(Path(tmp)/selected[0]['mask']))
                self.assertEqual(coverage.shape,(40,80))
                self.assertGreater(coverage.max(),32000)
                self.assertLess(coverage[:,0].max(),1000)

    @unittest.skipUnless(os.environ.get('SHOPPHOTO_TEST_MASKS'), 'optional cached detector, SAM and edge models')
    def test_actual_people_instances_preview_then_selected_full_resolution_mask(self):
        with tempfile.TemporaryDirectory() as tmp:
            output=Path(tmp)/'instances.json'
            source=Path(__file__).resolve().parents[1]/'docs/advanced-ai-features/ai-mask-scene.png'
            # Duplicate a focused crop: the old semantic-query implementation
            # merged these two people into one mask despite separate instances.
            with Image.open(source) as full:
                crop=full.convert('RGB').crop((850,300,1250,750))
            scene=Image.new('RGB',(800,450));scene.paste(crop,(0,0));scene.paste(crop,(400,0))
            input_path=Path(tmp)/'two-people.png';scene.save(input_path)
            request={'operation':'mask_instances','kind':'semantic_people','input':str(input_path),'output':str(output)}
            enhance.mask(request)
            detected=json.loads(output.read_text())
            self.assertGreaterEqual(len(detected['candidates']),2)
            first=detected['candidates'][0]
            self.assertIsNone(first['mask'])
            with Image.open(Path(tmp)/first['preview']) as preview:
                self.assertEqual(preview.mode,'RGBA')
                self.assertLessEqual(max(preview.size),160)
            enhance.mask({**request,'selected_ids':[first['id']],'selected_backend':detected['backend']})
            selected=json.loads(output.read_text())
            self.assertEqual([item['id'] for item in selected['candidates']],[first['id']])
            mask=np.asarray(Image.open(Path(tmp)/selected['candidates'][0]['mask']))
            self.assertEqual(mask.shape,(detected['height'],detected['width']))
            self.assertGreater(mask.max(),32000)
            self.assertLess(mask.min(),1000)
            self.assertGreater(len(np.unique(mask)),10)
            # Point refinement can retain the right person and exclude the
            # visually identical left person without selecting both together.
            point_output=Path(tmp)/'point-refined.png'
            enhance.mask({'operation':'mask','kind':'object','input':str(input_path),'output':str(point_output),
                          'points':[[.75,.25],[.75,.45],[.75,.72],[.25,.45]],'labels':[1,1,1,0]})
            with Image.open(point_output) as image:
                point_alpha=np.asarray(image,dtype=np.float32)/65535
            self.assertGreater(point_alpha[150:270,575:625].mean(),.8)
            self.assertLess(point_alpha[150:270,175:225].mean(),.01)

    def assert_actual_vehicle_instance(self, source, kind):
        with tempfile.TemporaryDirectory() as tmp:
            output=Path(tmp)/'instances.json'
            request={'operation':'mask_instances','kind':kind,'input':source,'output':str(output)}
            enhance.mask(request)
            detected=json.loads(output.read_text())
            self.assertGreaterEqual(len(detected['candidates']),1)
            first=detected['candidates'][0]
            with self.assertRaisesRegex(ValueError,'model changed'):
                enhance.mask({**request,'selected_ids':[first['id']],'selected_backend':'stale-backend'})
            enhance.mask({**request,'selected_ids':[first['id']],'selected_backend':detected['backend']})
            selected=json.loads(output.read_text())['candidates']
            self.assertEqual([item['id'] for item in selected],[first['id']])
            with Image.open(Path(tmp)/selected[0]['mask']) as image:
                coverage=np.asarray(image)
            self.assertEqual(coverage.shape,(detected['height'],detected['width']))
            self.assertGreater(coverage.max(),32000)
            self.assertLess(coverage.min(),1000)
            self.assertGreater(len(np.unique(coverage)),10)

    @unittest.skipUnless(os.environ.get('SHOPPHOTO_TEST_CAR_PHOTO'), 'optional licensed car fixture and cached models')
    def test_actual_car_instance_produces_nonempty_mask_and_rejects_stale_backend(self):
        self.assert_actual_vehicle_instance(os.environ['SHOPPHOTO_TEST_CAR_PHOTO'],'semantic_cars')

    @unittest.skipUnless(os.environ.get('SHOPPHOTO_TEST_MOTORCYCLE_PHOTO'), 'optional licensed motorcycle fixture and cached models')
    def test_actual_motorcycle_instance_produces_nonempty_mask_and_rejects_stale_backend(self):
        self.assert_actual_vehicle_instance(os.environ['SHOPPHOTO_TEST_MOTORCYCLE_PHOTO'],'semantic_motorcycles')

    def test_final_masks_reject_empty_nonfinite_and_misaligned_pixels(self):
        for alpha, size in [(np.zeros((2,3)), (3,2)), (np.full((2,3),1e-8), (3,2)), (np.full((2,3),np.nan), (3,2)), (np.full((2,3),np.inf), (3,2)), (np.ones((2,3)), (2,3))]:
            with self.assertRaises(ValueError):
                enhance.quantize_mask(alpha, size)
        pixels = enhance.quantize_mask(np.array([[0.0,0.5,1.0]]), (3,1))
        np.testing.assert_array_equal(pixels, [[0,32768,65535]])
        self.assertEqual(pixels.dtype,np.uint16)

    def test_text_prompts_and_coverage_reject_invalid_or_absent_selections(self):
        self.assertEqual(enhance.text_prompts(" the red car, trees "), ["the red car", "trees"])
        for value in (None, "", " ", "x"*257, "car,", ",".join(["car"]*9)):
            with self.assertRaises(ValueError): enhance.text_prompts(value)
        logits = torch.tensor([[[12.0,-12.0,-12.0]],[[-12.0,12.0,-12.0]]])
        np.testing.assert_allclose(enhance.text_coverage(logits), [[1.0,1.0,0.0]])
        for logits in (torch.full((1,2,2),-12.0), torch.full((1,2,2),float("nan"))):
            with self.assertRaises(ValueError): enhance.text_coverage(logits)

    @unittest.skipUnless(os.environ.get("SHOPPHOTO_TEST_TEXT_MASKS"), "optional cached CLIPSeg model")
    def test_actual_text_mask_preserves_dimensions_and_has_continuous_coverage(self):
        source = Path(__file__).resolve().parents[1]/"docs/advanced-ai-features/ai-mask-scene.png"
        with tempfile.TemporaryDirectory() as tmp:
            input_path, output = Path(tmp)/"input.png", Path(tmp)/"mask.png"
            image=Image.open(source).convert("RGB")
            image.thumbnail((512,512));image.save(input_path)
            enhance.mask({"input":str(input_path),"output":str(output),"kind":"text","prompt":"person"})
            coverage=np.asarray(Image.open(output))
            self.assertEqual(coverage.shape,(image.height,image.width))
            self.assertGreater(int(coverage.max()),32000)
            self.assertLess(int(coverage.min()),1000)
            self.assertGreater(len(np.unique(coverage)),10)

    def test_model_cache_reuses_and_bounds_live_entries(self):
        enhance._MODELS.clear()
        first = enhance.cached_model("first", object)
        self.assertIs(first, enhance.cached_model("first", lambda: self.fail("reloaded cached model")))
        enhance.cached_model("second", object)
        enhance.cached_model("third", object)
        enhance.cached_model("fourth", object)
        enhance.cached_model("fifth", object)
        self.assertEqual(len(enhance._MODELS), 4)
        self.assertNotIn("first", enhance._MODELS)
        enhance._MODELS.clear()

    def test_cache_enforces_byte_budget_lru_and_oversized_bypass(self):
        cache = enhance.MemoryCache(48, 4)
        first = cache.get("first", lambda: np.zeros(4, np.float32))
        cache.get("second", lambda: np.zeros(4, np.float32))
        self.assertIs(first, cache.get("first", lambda: self.fail("cached product reloaded")))
        cache.get("third", lambda: np.zeros(6, np.float32))
        self.assertIn("first", cache)
        self.assertNotIn("second", cache)
        self.assertEqual(cache.retained_bytes, 40)
        self.assertEqual(cache.get("oversized", lambda: np.zeros(100, np.float32)).nbytes, 400)
        self.assertNotIn("oversized", cache)
        self.assertEqual(cache.retained_bytes, 40)
        cache.clear()
        self.assertEqual(cache.retained_bytes, 0)

    def test_model_budget_counts_parameters_buffers_and_descriptor_weights(self):
        model = torch.nn.Linear(2, 3)
        model.register_buffer("extra", torch.zeros(4))
        from types import SimpleNamespace
        self.assertEqual(enhance.model_bytes((object(), model)), 52)
        self.assertEqual(enhance.model_bytes(SimpleNamespace(model=model)), 52)

    def test_uncompressed_gui_input_preserves_pixels_and_rejects_invalid_buffers(self):
        rgba = np.random.default_rng(42).integers(0,256,(17,23,4),dtype=np.uint8)
        with tempfile.TemporaryDirectory() as tmp:
            input_path, png = Path(tmp)/"input.rgba8", Path(tmp)/"input.png"
            input_path.write_bytes(rgba.tobytes())
            Image.fromarray(rgba).save(png)
            request = {"input":str(input_path),"input_format":"rgba8","width":23,"height":17}
            np.testing.assert_array_equal(np.asarray(enhance.load_rgb(request)), np.asarray(enhance.load_rgb({"input":str(png)})))
            for width,height in [(None,17),(0,17),(-1,17),(23.0,17),(True,17),(150_000_001,1),(23,16)]:
                with self.assertRaises(ValueError):
                    enhance.load_rgb({**request,"width":width,"height":height})
            input_path.write_bytes(b"truncated")
            with self.assertRaisesRegex(ValueError,"size"):
                enhance.load_rgb(request)
            with self.assertRaisesRegex(ValueError,"format"):
                enhance.load_rgb({"input":str(png),"input_format":"unsupported"})

    def test_image_cache_uses_pixels_dimensions_model_revision_and_device(self):
        enhance._PRODUCTS.clear()
        image = Image.new("RGB", (2, 3))
        key = enhance.image_key(image)
        product = enhance._PRODUCTS.get((key, enhance.model_key(enhance.SAM, "cpu")), lambda: np.ones(1))
        self.assertEqual(enhance.image_key(image.copy()), key)
        self.assertIs(product, enhance._PRODUCTS.get((key, enhance.model_key(enhance.SAM, "cpu")), lambda: self.fail("identical pixels reloaded")))
        changed_device = enhance._PRODUCTS.get((key, enhance.model_key(enhance.SAM, "mps")), lambda: np.zeros(1))
        self.assertIsNot(product, changed_device)
        with mock.patch.dict(enhance.REVISIONS, {enhance.SAM: "different-revision"}):
            replacement = enhance._PRODUCTS.get((key, enhance.model_key(enhance.SAM, "cpu")), lambda: np.zeros(1))
            self.assertIsNot(product, replacement)
        self.assertNotEqual(enhance.image_key(Image.new("RGB", (3, 2))), key)
        self.assertEqual(len(enhance._PRODUCTS), 0)
        changed = image.copy()
        changed.putpixel((0, 0), (1, 0, 0))
        self.assertNotEqual(enhance.image_key(changed), key)
        enhance._PRODUCTS.clear()

    @unittest.skipUnless(os.environ.get("SHOPPHOTO_TEST_MASKS"), "optional cached-model integration")
    def test_actual_sam_cache_matches_fresh_inference_and_invalidates_changed_pixels(self):
        from transformers import SamModel
        calls = []
        original = SamModel.get_image_embeddings
        def encode(model, *args, **kwargs):
            calls.append(1)
            return original(model, *args, **kwargs)
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(SamModel, "get_image_embeddings", encode):
            input_path, duplicate, output = Path(tmp)/"input.png", Path(tmp)/"duplicate.png", Path(tmp)/"mask.png"
            source = Path(__file__).resolve().parents[1]/"docs/advanced-ai-features/ai-mask-scene.png"
            image = Image.open(source).convert("RGB")
            image.thumbnail((512,512))
            image.save(input_path)
            image.save(duplicate)
            enhance._PRODUCTS.clear()
            request = {"input":str(input_path),"output":str(output),"kind":"object","points":[[0.5,0.5]]}
            enhance.mask(request)
            baseline = np.asarray(Image.open(output)).copy()
            enhance.mask({**request, "input":str(duplicate)})
            np.testing.assert_array_equal(baseline, np.asarray(Image.open(output)))
            self.assertEqual(len(calls), 1, "temp filename changes must retain the image encoding")
            request["points"] = [[0.5,0.5],[0.95,0.05]]
            request["labels"] = [1,0]
            enhance.mask(request)
            prompted = np.asarray(Image.open(output)).copy()
            self.assertEqual(len(calls), 1, "changed point prompts must reuse the image encoding")
            enhance._PRODUCTS.clear()
            enhance.mask(request)
            np.testing.assert_array_equal(prompted, np.asarray(Image.open(output)))
            self.assertEqual(len(calls), 2)
            pixel = image.getpixel((0,0))
            image.putpixel((0,0), ((pixel[0]+1)%256,pixel[1],pixel[2]))
            image.save(input_path)
            enhance.mask(request)
            self.assertEqual(len(calls), 3, "changed pixels in the same file must re-encode")
            raw = Path(tmp)/"input.rgba8"
            raw.write_bytes(image.convert("RGBA").tobytes())
            expected = np.asarray(Image.open(output)).copy()
            enhance.mask({**request,"input":str(raw),"input_format":"rgba8","width":image.width,"height":image.height})
            np.testing.assert_array_equal(expected, np.asarray(Image.open(output)))
            self.assertEqual(len(calls), 3, "RGBA transport must share the PNG pixel cache")

    @unittest.skipUnless(os.environ.get("SHOPPHOTO_TEST_MASKS"), "optional cached-model integration")
    def test_actual_depth_cache_reuses_prediction_for_changed_thresholds(self):
        from transformers import DepthAnythingForDepthEstimation
        calls = []
        forward = DepthAnythingForDepthEstimation.forward
        def predict(model, *args, **kwargs):
            calls.append(1)
            return forward(model, *args, **kwargs)
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(DepthAnythingForDepthEstimation, "forward", predict):
            input_path, output = Path(tmp)/"input.png", Path(tmp)/"mask.png"
            source = Path(__file__).resolve().parents[1]/"docs/advanced-ai-features/ai-mask-scene.png"
            image = Image.open(source).convert("RGB")
            image.thumbnail((512,512)); image.save(input_path)
            enhance._PRODUCTS.clear()
            request = {"input":str(input_path),"output":str(output),"kind":"depth","near":0.5,"far":1.0}
            enhance.mask(request)
            original = np.asarray(Image.open(output)).copy()
            request["near"] = 0.2
            enhance.mask(request)
            changed = np.asarray(Image.open(output)).copy()
            self.assertEqual(len(calls), 1)
            self.assertTrue(np.any(original != changed))
            enhance._PRODUCTS.clear()
            enhance.mask(request)
            np.testing.assert_array_equal(changed, np.asarray(Image.open(output)))
            self.assertEqual(len(calls), 2)

    @unittest.skipUnless(os.environ.get("SHOPPHOTO_TEST_SEMANTIC_SCENE"), "optional cached semantic model and fixture")
    def test_actual_semantic_cache_reuses_predictions_between_people_and_sky(self):
        from transformers import MaskFormerForInstanceSegmentation
        calls = []
        forward = MaskFormerForInstanceSegmentation.forward
        def predict(model, *args, **kwargs):
            calls.append(1)
            return forward(model, *args, **kwargs)
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(MaskFormerForInstanceSegmentation, "forward", predict):
            input_path, output = Path(tmp)/"input.png", Path(tmp)/"mask.png"
            image = Image.open(os.environ["SHOPPHOTO_TEST_SEMANTIC_SCENE"]).convert("RGB")
            image.thumbnail((512,512)); image.save(input_path)
            enhance._PRODUCTS.clear()
            for kind in enhance.SEMANTIC_SUBJECTS:
                request = {"input":str(input_path),"output":str(output),"kind":kind}
                try:
                    enhance.mask(request)
                except ValueError as error:
                    self.assertIn("No confidently detected", str(error))
                    continue
                expected = np.asarray(Image.open(output)).copy()
                enhance.mask(request)
                np.testing.assert_array_equal(expected, np.asarray(Image.open(output)))
            self.assertEqual(len(calls), 1, "class changes must reuse the same image prediction")

    def test_persistent_worker_reports_errors_without_exiting(self):
        with tempfile.TemporaryDirectory() as tmp:
            child = subprocess.Popen([sys.executable, str(Path(enhance.__file__)), "--serve"], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, text=True)
            try:
                # Invalid protocol lines and unwritable response destinations
                # must not prevent subsequent valid requests from completing.
                for line in ("not json", "[]", "null", "{}", '{"_response": null}',
                             *(json.dumps({"operation": "invalid", "_response": path}) for path in
                               ("", "/", "invalid\0path", str(Path(tmp)/"missing"/"response.json")))):
                    child.stdin.write(line+"\n")
                child.stdin.flush()
                for index in range(2):
                    response = Path(tmp)/f"response-{index}.json"
                    child.stdin.write(json.dumps({"operation": "invalid", "_response": str(response)})+"\n")
                    child.stdin.flush()
                    deadline = time.monotonic()+10
                    while not response.exists() and time.monotonic() < deadline:
                        self.assertIsNone(child.poll())
                        time.sleep(0.02)
                    self.assertTrue(response.exists(), "worker never replied")
                    self.assertEqual(json.loads(response.read_text()), {"ok": False, "error": "Unknown enhancement operation"})
                child.stdin.close()
                self.assertEqual(child.wait(timeout=10), 0)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()

    def test_semantic_coverage_uses_class_probabilities_and_ignores_null_queries(self):
        from types import SimpleNamespace
        outputs = SimpleNamespace(class_queries_logits=torch.tensor([[[12.0,-12.0,-12.0],[-12.0,12.0,-12.0],[-12.0,-12.0,12.0]]]), masks_queries_logits=torch.tensor([[[[12.0,-12.0]],[[-12.0,12.0]],[[12.0,12.0]]]]))
        alpha = enhance.semantic_alpha(outputs,0).numpy()[0,0]
        self.assertGreater(alpha[0],0.999)
        self.assertLess(alpha[1],0.001)
        with self.assertRaises(ValueError): enhance.semantic_alpha(outputs,2)

    def test_invalid_prompts_are_rejected_before_loading_a_network(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/"input.png"
            Image.new("RGB",(16,16)).save(path)
            for points,labels in [([],[]),([[0.5,0.5]],[0]),([[float("nan"),0.5]],[1]),([[0.5,0.5]],[1,0])]:
                with self.assertRaisesRegex(ValueError,"prompts"):
                    enhance.mask({"input":str(path),"output":str(Path(tmp)/"mask.png"),"kind":"object","points":points,"labels":labels})

    def test_face_input_dimensions_are_checked_before_decoding_pixels(self):
        source = mock.MagicMock()
        source.width, source.height = 150_000_001, 1
        source.__enter__.return_value = source
        with mock.patch("PIL.Image.open", return_value=source):
            with self.assertRaisesRegex(ValueError, "150 MP"):
                enhance.analyze_faces({"input": "oversized.png", "output": "unused.json"})
        source.convert.assert_not_called()

    def test_empty_face_preview_is_valid_without_a_blink_verdict(self):
        with tempfile.TemporaryDirectory() as tmp:
            source,output=Path(tmp)/"empty.png",Path(tmp)/"faces.json"
            Image.new("RGB",(128,128),(127,127,127)).save(source)
            enhance.analyze_faces({"input":str(source),"output":str(output)})
            self.assertEqual(json.loads(output.read_text()),{"faces":0,"eye_regions":0,"weakest_eye_detail":None})

    @unittest.skipUnless(os.environ.get("SHOPPHOTO_TEST_FACE_PHOTO"), "optional actual face fixture")
    def test_actual_face_eye_detector_and_detail(self):
        with tempfile.TemporaryDirectory() as tmp:
            output=Path(tmp)/"faces.json"
            enhance.analyze_faces({"input":os.environ["SHOPPHOTO_TEST_FACE_PHOTO"],"output":str(output)})
            result=json.loads(output.read_text())
            self.assertGreaterEqual(result["faces"],1)
            self.assertGreaterEqual(result["eye_regions"],2)
            self.assertGreaterEqual(result["weakest_eye_detail"],0)
            self.assertLessEqual(result["weakest_eye_detail"],100)

    @unittest.skipUnless(os.environ.get("SHOPPHOTO_TEST_SEMANTIC_SCENE") and os.environ.get("SHOPPHOTO_TEST_FACE_PHOTO"), "optional cached semantic model and fixtures")
    def test_actual_automatic_sky_and_people_masks(self):
        with tempfile.TemporaryDirectory() as tmp:
            for kind,source in [("semantic_sky",os.environ["SHOPPHOTO_TEST_SEMANTIC_SCENE"]),("semantic_people",os.environ["SHOPPHOTO_TEST_FACE_PHOTO"])]:
                output=Path(tmp)/f"{kind}.png"
                enhance.mask({"input":source,"output":str(output),"kind":kind})
                mask=np.asarray(Image.open(output))
                with Image.open(source) as original:
                    self.assertEqual(mask.shape,(original.height,original.width))
                self.assertGreater(int(mask.max()),32000)
                self.assertLess(int(mask.min()),1000)

    def test_tiled_sr_odd_dimensions_and_seams(self):
        rgb = np.random.default_rng(42).random((157, 273, 3), dtype=np.float32)
        calls = {}
        class CountingNearest(Nearest):
            def __call__(self, value):
                calls.setdefault(batch, []).append(value.shape[0])
                return super().__call__(value)
        for batch in (1,2,4):
            output = enhance.tiled_sr(rgb, CountingNearest(), 2, tile=64, halo=16, batch_size=batch)
            np.testing.assert_array_equal(output, rgb.repeat(2, axis=0).repeat(2, axis=1))
            self.assertLessEqual(max(calls[batch]), batch)
        self.assertLess(len(calls[2]), len(calls[1]))
        self.assertLess(len(calls[4]), len(calls[1]))

    @unittest.skipUnless(os.environ.get("SHOPPHOTO_TEST_MASKS"), "optional cached-model integration")
    def test_actual_matting_batches_match_single_tiles(self):
        source = Path(__file__).resolve().parents[1]/"docs/advanced-ai-features/ai-mask-scene.png"
        rgb = np.asarray(Image.open(source).convert("RGB"))
        h, w = rgb.shape[:2]
        coarse = np.linspace(0.1, 0.9, w, dtype=np.float32)[None, :].repeat(h, axis=0)
        where = enhance.device(torch)
        expected = enhance.matte_boundaries(rgb, coarse, where, batch_size=1)
        for batch in (2,4):
            actual = enhance.matte_boundaries(rgb, coarse, where, batch_size=batch)
            difference = np.abs(np.round(expected*65535).astype(np.int32)-np.round(actual*65535).astype(np.int32))
            self.assertLessEqual(int(difference.max()), 2)

    @unittest.skipUnless(os.environ.get("SHOPPHOTO_TEST_SR"), "optional super-resolution checkpoint")
    def test_actual_sr_batches_preserve_texture_and_seams(self):
        from spandrel import ModelLoader
        source = Path(__file__).resolve().parents[1]/"docs/advanced-ai-features/ai-mask-scene.png"
        image = Image.open(source).convert("RGB")
        image.thumbnail((640,384))
        rgb = np.asarray(image).astype(np.float32)/255
        model = ModelLoader().load_from_file(os.environ["SHOPPHOTO_TEST_SR"]).to(enhance.device(torch)).eval().float()
        if model.architecture.id != "SwinIR":
            self.skipTest("batch acceptance currently covers SwinIR")
        expected = enhance.tiled_sr(rgb, model, model.scale, batch_size=1)
        for batch in (2,4):
            actual = enhance.tiled_sr(rgb, model, model.scale, batch_size=batch)
            difference = np.abs(np.round(expected*65535).astype(np.int32)-np.round(actual*65535).astype(np.int32))
            self.assertLessEqual(int(difference.max()), 2)

    def test_refinement_preserves_interiors_and_dimensions(self):
        rgb = np.zeros((77, 103, 3), np.uint8)
        rgb[:, 51:] = 255
        for value in (0, 1):
            result = enhance.refine_alpha(rgb, np.full((11, 13), value, np.float32))
            self.assertEqual(result.shape, rgb.shape[:2])
            np.testing.assert_allclose(result, value, atol=1e-6)
        result = enhance.refine_alpha(rgb, np.linspace(0, 1, 13, dtype=np.float32)[None, :].repeat(11, axis=0))
        self.assertTrue(np.isfinite(result).all())
        self.assertGreaterEqual(result.min(), 0)
        self.assertLessEqual(result.max(), 1)

    @unittest.skipUnless(os.environ.get("SHOPPHOTO_TEST_MASKS"), "optional cached-model integration")
    def test_actual_masks_on_mps_or_cpu(self):
        source = Path(__file__).resolve().parents[1]/"docs/advanced-ai-features/ai-mask-scene.png"
        with tempfile.TemporaryDirectory() as tmp:
            input_path = Path(tmp)/"input.png"
            image = Image.open(source).convert("RGB")
            image.thumbnail((512, 512))
            image.save(input_path)
            for kind in ("object", "depth"):
                output = Path(tmp)/f"{kind}.png"
                enhance.mask({"input": str(input_path), "output": str(output), "kind": kind, "points": [[0.5, 0.5]]})
                mask = np.asarray(Image.open(output))
                self.assertEqual(mask.shape, (image.height, image.width))
                self.assertGreater(int(mask.max()), int(mask.min()))
            # Positive and negative prompts run through the actual SAM+matte path.
            output=Path(tmp)/"prompted.png"
            enhance.mask({"input":str(input_path),"output":str(output),"kind":"object","points":[[0.5,0.5],[0.95,0.05]],"labels":[1,0]})
            prompted=np.asarray(Image.open(output))
            self.assertEqual(prompted.shape,(image.height,image.width))
            self.assertGreater(int(prompted.max()),int(prompted.min()))

    @unittest.skipUnless(os.environ.get("SHOPPHOTO_TEST_SR"), "optional super-resolution checkpoint")
    def test_actual_sr_and_hdr_highlight_preservation(self):
        import cv2
        with tempfile.TemporaryDirectory() as tmp:
            input_path, output = Path(tmp)/"input.png", Path(tmp)/"output.png"
            for hdr in (False, True):
                # Constant PQ at 0.75 is a bright HDR patch, above SDR white.
                pixels = np.full((35, 43, 3), round(0.75*65535), np.uint16)
                cv2.imwrite(str(input_path), pixels)
                enhance.upscale({"input": str(input_path), "output": str(output), "model": os.environ["SHOPPHOTO_TEST_SR"], "hdr": hdr})
                result = cv2.imread(str(output), cv2.IMREAD_UNCHANGED)
                self.assertEqual(result.shape, (70, 86, 3))
                self.assertEqual(result.dtype, np.uint16)
                if hdr:
                    self.assertLess(np.abs(result.astype(np.float32)-pixels[0,0]).max(), 900)
                    np.testing.assert_array_equal(result[:, :, 0], result[:, :, 1])


if __name__ == "__main__":
    unittest.main()
