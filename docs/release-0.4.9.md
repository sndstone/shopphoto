# ShopPhoto 0.4.9 — Golden Hour and Neon Night presets

Two new creative looks bring the built-in collection to 13 presets:

- **Golden Hour Radiance:** warm highlights, open shadows and restrained color enhancement without the muted film look.
- **Neon Night:** selective aqua, blue and magenta enhancement with controlled highlights and deeper shadows.

Both presets work in SDR. When HDR is already enabled, they also adjust highlight rolloff. They never enable HDR or change the output target, headroom or scene reference white. Disabled HDR settings remain untouched.

Preset switching, hover previews and thumbnails now respect HDR settings changed after applying a preset. Resetting edits no longer lets a stale preset baseline return through previews. The Amount control retains stable interpolation. Dutch names and descriptions and the bundled research notebook are updated.

## macOS download

`ShopPhoto-0.4.9.dmg` contains a universal app for Apple Silicon and Intel, the photography guide and research notebook. This build is ad-hoc signed, not Apple-notarized; macOS may require explicit approval to open it.

## Validation

Core tests: 148 passed, 1 ignored. Five targeted app tests cover preset amount, HDR state changes, reset behavior, translations and thumbnail downsampling. Formatting passes. Visual calibration of the new looks on representative sunset/neon photographs and a physical HDR display remains outstanding.
