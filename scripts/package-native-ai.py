#!/usr/bin/env python3
"""Verify and optionally package an explicit, complete native enhancement set.

Uses only the standard library. Public builds remain model-free unless the
packager explicitly supplies a source. Never reads camera-denoise directories.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil


def verify(bundle):
    manifest_path = bundle / "manifest.json"
    if manifest_path.stat().st_size > 1_000_000:
        raise ValueError(f"oversized manifest: {bundle}")
    manifest = json.loads(manifest_path.read_text())
    if manifest.get("version") != 1 or manifest.get("id") != bundle.name:
        raise ValueError(f"invalid bundle identity: {bundle}")
    for required in ("license", "source", "revision", "inputs", "outputs", "files"):
        if not manifest.get(required):
            raise ValueError(f"missing {required}: {bundle}")
    model = Path(manifest["model"])
    if model.is_absolute() or ".." in model.parts or model.suffix != ".mlmodelc":
        raise ValueError(f"unsafe model path: {bundle}")
    pinned = manifest["files"]
    for name, expected in pinned.items():
        relative = Path(name)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(f"unsafe file path: {bundle}/{name}")
        file = bundle / relative
        if file.is_symlink() or not file.resolve().is_relative_to(bundle.resolve()):
            raise ValueError(f"model symlink: {file}")
        with file.open("rb") as stream:
            digest = hashlib.sha256()
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
            actual = digest.hexdigest()
        if actual != expected:
            raise ValueError(f"checksum mismatch: {file}")
    graph_files = [p for p in (bundle / model).rglob("*") if p.is_file()]
    if not graph_files or any(str(p.relative_to(bundle)) not in pinned for p in graph_files):
        raise ValueError(f"missing or unpinned graph files: {bundle}")
    if not (bundle / "LICENSE.txt").is_file():
        raise ValueError(f"license notice missing: {bundle}")
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("--copy-to", type=Path)
    args = parser.parse_args()
    sky = "oneformer-ade" if (args.source / "oneformer-ade/manifest.json").is_file() else "maskformer-ade"
    names = [sky, "ssdlite-coco", "slimsam-encoder", "slimsam-box", "slimsam-points", "vitmatte-small", "swinir-2x", "swinir-4x"]
    manifests = [verify(args.source / name) for name in names]
    if args.copy_to:
        if args.copy_to.exists():
            parser.error("destination already exists; package into a fresh directory")
        args.copy_to.mkdir(parents=True)
        try:
            for name in names:
                shutil.copytree(args.source / name, args.copy_to / name)
                verify(args.copy_to / name)
        except Exception:
            shutil.rmtree(args.copy_to)
            raise
    print(json.dumps({"bundles": names, "licenses": {m["id"]: m["license"] for m in manifests}}, indent=2))


if __name__ == "__main__":
    main()
