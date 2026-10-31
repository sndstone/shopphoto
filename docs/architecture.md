# As-built architecture

Dependency direction remains `sp-app → {sp-pipeline, sp-catalog, sp-export} → {sp-gpu, sp-raw} → sp-core`.

RAW decode produces an `Arc<RawImage>` so the interactive and export renderers share a 100 MP CFA without copying. Interactive edits run on a viewport-bounded proxy; zoom/loupe and full export use padded Malvar tiles. The temporary Bayer-superpixel texture is released after proxy construction and recreated only for an exact 2×2 export. All color/detail work is scene-linear Rec.2020 f16 until the final SDR/PQ output transform.

The UI thread drains decode, thumbnail, export, calibration, catalog, and update channels. SQLite uses short-lived WAL connections, while recursive scans stay on a dedicated worker. Sidecars remain authoritative; catalog rows are query indexes.
