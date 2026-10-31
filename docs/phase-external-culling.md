# External recipes and advisory culling analysis

Tools → External editor now offers saved photographic round-trip recipes. Each
recipe carries a name, an optional executable/application path, one argument per
line with a `{file}` placeholder, TIFF bit depth, and sRGB, Display P3, Adobe RGB
or Rec.2020 output. Argument strings are passed directly to the process; they
are not shell commands. Recipes are stored in the ShopPhoto configuration
directory. The existing default editor remains available.

External-file monitoring waits for two identical file signatures and a valid
image decode before accepting a save. It continues monitoring later saves for
up to twelve hours, refreshes the companion's thumbnail and open image, and
retains its relationship with the source. A menu control stops monitoring;
starting another external round trip replaces the previous monitor. Only the
first save switches to the Library. The original photograph remains untouched.

Tools → Library & offline → Analyze selected previews for culling computes
source-preview edge detail, white/black clipping percentages, a perceptual
difference hash and a luminance distribution. Library can sort analyzed photos
by detail and show similar analyzed photos from a photo's context menu. Pinned
offline originals work too. Side-by-side culling displays preview detail,
highlight clipping and approximate similarity to its first photograph.

These scores are advisory. Subject texture, preview size and camera sharpening
affect detail; low detail is not proof of missed focus. Similarity detects
roughly matching framing and tone, not people or semantic subjects. Differences
in crop, rotation, lighting or camera previews can reduce similarity. Inspect
focus at 100% before deciding. Analysis never changes ratings, rejects files or
deletes photographs. Library analysis is session-only and explicitly requested.

Tests verify literal argument handling including shell-looking filenames,
versioned recipe serialization, repeated stable-save detection, edge-detail
ordering, clipping counts, similarity behavior and malformed preview refusal.

Optional **Analyze selected faces / eye detail** uses the installed Python/OpenCV
runtime. It detects up to 32 faces on a preview capped at 1024 pixels and reports
at most two nonoverlapping eye regions per face, with regional edge-detail scores.
Faces/eyes that are not detected receive no invented score. These indicators are
advisory and do not assess blinking or closed eyes. Actual runtime acceptance
detected a face and two eyes in the public NASA astronaut portrait; inspect the
original at 100% for a photographic decision. No recognition identity is stored.

Four-up culling now binds independent float proxy/tile textures on HDR surfaces.
Its SDR fallback remains available. A real-GPU regression renders eight separate
bindings and UV regions, checks exact half-float samples above diffuse white,
and guards against accidentally showing another cell's texture or uniforms.
