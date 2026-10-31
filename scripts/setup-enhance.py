#!/usr/bin/env python3
"""Install the optional enhancement runtime, separate from AI denoise.

Run with Python 3.11 or newer. Use --no-models to defer mask downloads.
Upscale checkpoints are selected in the application; no diffusion is installed.
"""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import venv
import hashlib
import shutil
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--no-models", action="store_true")
    parser.add_argument("--directory", type=Path)
    parser.add_argument("--text", action="store_true", help="Also install optional CLIPSeg text selection and edge matting models")
    parser.add_argument("--semantic", action="store_true", help="Also install optional CC-BY-NC MaskFormer people/sky model; review its license before use")
    parser.add_argument("--upscale-swinir", action="store_true", help="Also install the official non-GAN SwinIR-M 2x checkpoint (64 MiB)")
    parser.add_argument("--upscale-swinir-4x", action="store_true", help="Also install the official non-GAN SwinIR-M 4x checkpoint (65 MiB)")
    args = parser.parse_args()
    if sys.version_info < (3, 11):
        parser.error("Python 3.11+ is required")
    if sys.platform == "darwin":
        data = Path.home() / "Library/Application Support"
    elif sys.platform == "win32":
        data = Path(os.environ["LOCALAPPDATA"])
    else:
        data = Path(os.environ.get("XDG_DATA_HOME", str(Path.home()/".local/share")))
    root = args.directory or data/"shopphoto/enhance-venv"
    # Refresh an existing interpreter after an OS/Homebrew Python upgrade;
    # preserve installed packages and cached model weights.
    venv.EnvBuilder(with_pip=True, upgrade=root.exists()).create(root)
    # EnvBuilder(upgrade=True) refreshes the versioned executable but leaves
    # existing POSIX python/python3 copies unchanged. Those copies can still
    # reference a Homebrew framework that has been removed.
    if sys.platform != "win32":
        versioned = root/"bin"/f"python{sys.version_info.major}.{sys.version_info.minor}"
        for name in ("python", "python3"):
            destination = root/"bin"/name
            temporary = destination.with_name(name+".runtime-update")
            shutil.copy2(versioned, temporary)
            temporary.replace(destination)
    python = root/("Scripts/python.exe" if sys.platform == "win32" else "bin/python")
    scripts = Path(__file__).resolve().parent
    subprocess.run([str(python), "-m", "pip", "install", "-r", str(scripts/"enhance-requirements.txt")], check=True)
    if not args.no_models:
        subprocess.run([str(python), str(scripts/"enhance.py"), "--download-masks"], check=True)
    if args.text:
        subprocess.run([str(python), str(scripts/"enhance.py"), "--download-text"], check=True)
    if args.semantic:
        subprocess.run([str(python), str(scripts/"enhance.py"), "--download-semantic"], check=True)
    for scale, enabled, expected in [
        (2, args.upscale_swinir, "2032ebf8f401dd3ce2fae5f3852117cb72101ec6ed8358faa64c2a3fa09ed4ac"),
        (4, args.upscale_swinir_4x, "4e78e33f22c1aa8a773db0cf4a7381bae97c2362c717f155439ebc690cbd9215"),
    ]:
        if not enabled:
            continue
        destination = data/f"shopphoto/enhance-models/SwinIR-M-x{scale}.pth"
        if not destination.exists() or hashlib.sha256(destination.read_bytes()).hexdigest() != expected:
            url = f"https://github.com/JingyunLiang/SwinIR/releases/download/v0.0/001_classicalSR_DF2K_s64w8_SwinIR-M_x{scale}.pth"
            with urllib.request.urlopen(url, timeout=120) as response:
                content = response.read(80 * 1024 * 1024)
            if hashlib.sha256(content).hexdigest() != expected:
                raise ValueError("Upscale checkpoint checksum mismatch; nothing installed")
            destination.parent.mkdir(parents=True, exist_ok=True)
            temporary = destination.with_suffix(".download")
            temporary.write_bytes(content)
            temporary.replace(destination)
        print(f"Verified SwinIR {scale}x checkpoint: {destination}")
    print(f"Enhancement runtime ready: {python}")
    if args.directory:
        print(f"Set SHOPPHOTO_ENHANCE_PYTHON={python}")


if __name__ == "__main__":
    main()
