# Phase 9 — Packaging and release

- `scripts/bundle-macos.sh`: arm64+x86_64 universal app, iconset, document associations, hardened codesign hook, DMG, optional notarytool submission.
- `packaging/windows/ShopPhoto.nsi`: NSIS installer and 3FR/ARW/FFF/DNG associations.
- `scripts/package-linux.sh` and Flatpak manifest: AppDir/AppImage and Flatpak inputs.
- Tag-driven GitHub release workflow publishes a universal DMG, NSIS installer, and AppImage rather than loose binaries.
- First-run folder/RAW guide, dismissible first-edit coaching card, in-app shortcut sheet, and at-most-daily semver update check (enabled with compile-time `SHOPPHOTO_UPDATE_URL`). No auto-download.

Signing and notarization activate only when the release environment supplies its private credentials. Repository and CI builds remain reproducible without secrets.
