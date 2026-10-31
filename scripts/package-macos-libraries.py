#!/usr/bin/env python3
"""Copy and relocate non-system Mach-O dependencies inside a generated app."""
import shutil
import plistlib
import platform
import re
import subprocess
import sys
from pathlib import Path


def run(*args):
    return subprocess.check_output(args, text=True)


def dependencies(path):
    return [line.strip().split(" (compatibility", 1)[0]
            for line in run("otool", "-L", str(path)).splitlines()
            if line.startswith("\t")]


def package(app):
    executable = app / "Contents/MacOS/ShopPhoto"
    frameworks = app / "Contents/Frameworks"
    licenses = app / "Contents/Resources/Codec Licenses"
    frameworks.mkdir(parents=True, exist_ok=True)
    licenses.mkdir(parents=True, exist_ok=True)
    copied = {}
    pending = [(executable, executable)]
    while pending:
        source, destination = pending.pop()
        for reference in dependencies(source):
            if reference.startswith(("/usr/lib/", "/System/Library/")):
                continue
            dependency = Path(reference)
            if not dependency.is_absolute():
                if reference.startswith("@loader_path/"):
                    dependency = source.parent / reference.removeprefix("@loader_path/")
                elif reference.startswith("@rpath/"):
                    dependency = source.parent / reference.removeprefix("@rpath/")
                else:
                    raise RuntimeError(f"Unresolved dependency: {reference} in {source}")
            dependency = dependency.resolve(strict=True)
            if dependency == source.resolve():
                continue  # install name of this library, not a dependency
            name = Path(reference).name
            if name in copied and copied[name] != dependency:
                raise RuntimeError(f"Conflicting library names: {name}")
            target = frameworks / name
            if name not in copied:
                copied[name] = dependency
                shutil.copy2(dependency, target)
                target.chmod(0o755)
                subprocess.check_call(["install_name_tool", "-id", f"@rpath/{name}", str(target)])
                pending.append((dependency, target))
                # Retain installed dependency licenses, including linked codecs.
                root = dependency.parent.parent
                for candidate in ("COPYING", "LICENSE", "LICENSE.md", "LICENSE.txt", "COPYING.LIB"):
                    license_path = root / candidate
                    if license_path.is_file():
                        shutil.copy2(license_path, licenses / f"{name}-{candidate}")
                for license_path in (root / "share").glob("licenses/**/*"):
                    if license_path.is_file():
                        shutil.copy2(license_path, licenses / f"{name}-{license_path.name}")
            relocated = (f"@executable_path/../Frameworks/{name}"
                         if destination == executable else f"@loader_path/{name}")
            subprocess.check_call(["install_name_tool", "-change", reference, relocated, str(destination)])
    for library in frameworks.iterdir():
        if library.is_file():
            subprocess.check_call(["codesign", "--force", "--sign", "-", str(library)])
    for path in [executable, *frameworks.iterdir()]:
        for reference in dependencies(path):
            if reference.startswith(("/opt/", "/usr/local/")):
                raise RuntimeError(f"Unpackaged dependency: {reference}")
    # Homebrew libraries may require a newer OS than the Rust executable.
    # Declare that requirement for the slice which actually links them instead
    # of producing a bundle that Launch Services falsely considers compatible.
    plist = app / "Contents/Info.plist"
    if plist.exists():
        info = plistlib.loads(plist.read_bytes())
        host_arch = platform.machine()
        floor = tuple(int(v) for v in info.get("LSMinimumSystemVersion", "12.0").split("."))
        for library in frameworks.iterdir():
            load_commands = run("otool", "-l", str(library))
            for minimum in re.findall(r"\bminos\s+([\d.]+)", load_commands):
                floor = max(floor, tuple(int(v) for v in minimum.split(".")))
        info.setdefault("LSMinimumSystemVersionByArchitecture", {})[host_arch] = ".".join(map(str, floor))
        plist.write_bytes(plistlib.dumps(info, sort_keys=False))
        (app / "Contents/Resources/Native Codec Requirements.txt").write_text(
            f"Native HEIC/AVIF input in the {host_arch} slice requires macOS {'.'.join(map(str, floor))} or later.\n"
            "The other universal slice retains the standard raster codecs.\n"
        )
        print(f"Native-codec slice {host_arch} requires macOS {'.'.join(map(str, floor))} or later")
    print(f"Packaged {len(copied)} native libraries in {frameworks}")


if __name__ == "__main__":
    package(Path(sys.argv[1]))
