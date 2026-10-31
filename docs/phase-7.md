# Phase 7 — Library and catalog

- SQLite WAL catalog with versioned migrations for photos, folders, collections, membership, and export presets.
- Recursive and watched incremental scans mirror rating/flag/edit state from sidecars, remove missing paths, and update camera/capture metadata as thumbnails decode; a catalog can be deleted and rebuilt without losing edits.
- Virtualized grid with rating/flag/type filters, capture/name/rating sorts, edited badges, and priority thumbnail requests.
- Decode pool size is physical cores / 2; viewport generations cancel scrolled-away work and evict offscreen GPU thumbnail textures.
- Path/mtime content-versioned PNG thumbnails are connected to a 2 GB LRU disk cache; debounced notify watchers trigger reliable re-stat/rescans.
- Ratings 0–5 and P/X persist to sidecar and DB. Collections accept thumbnail drag/drop and context-menu adds.
- Named export presets save to JSON for backward compatibility and mirror into SQLite; startup can recover from either store.
