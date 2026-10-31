# Library versions, import review, stacks and derivative provenance

Virtual-copy sidecars now carry an optional human-readable version name while
retaining their existing immutable identifier and filename. Develop's History
card can rename a version or send just that version to the OS Trash. Library
photos show their version count and a Versions menu that opens the chosen edit.
Catalog scans retain version descriptors without decompressing every mask.
Renaming or moving an original carries every version, and offline pinning copies
versions as well as the original. A failed remote mirror leaves local edits safe.

Import now offers a selectable contact sheet with capture dates, source-relative
filenames and preview diagnostics. Imports accept supported RAW and rendered
photographs. Date folders use `YYYY/YYYY-MM-DD`, with `Undated` for missing or
invalid capture dates; `{date}` is available in filename patterns. Primary and
backup copies are verified. Source XMP, primary edits and virtual copies are
preserved. Nonempty import metadata fields replace the corresponding source
fields, keywords are merged, and blank fields preserve source values. Creator,
copyright, usage rights, contact email, caption and keyword templates can be
saved and loaded as versioned JSON files. Backups receive the applied edit and
metadata as well as the original bytes.

Imports stage a complete photo, edits and applied template before publishing.
Durable manifests in `.shopphoto-import` retain verified bytes until primary
and backup copies are complete. **Import / resume** replays unfinished work
before duplicate detection. A destination lock prevents concurrent imports;
atomic non-replacing publication keeps existing files safe. Changed published
files cause an explicit conflict rather than being overwritten. Resume uses
the same source, destination and backup configuration. Staging needs room for
one extra original; interrupted staging before its manifest can leave temporary
files that may be removed from `.shopphoto-import` after imports have stopped.

Library selections can be grouped into persistent manual stacks using Tools →
Library & offline or the grid context menu. Stacks can be expanded, collapsed,
assigned a different cover and dissolved without moving or deleting files.
Filtering a collapsed stack uses a matching member if its cover is filtered
out. Stack links follow catalog IDs through rename/relink and catalog backups.

**Review automatic photo stacks** proposes bounded capture-time groups using
folder/camera/lens/timezone, or exact-stem RAW+JPEG pairs with one unambiguous
RAW. The user reviews and selects proposals before creating stacks. Existing
stacks are excluded; no shooting intent is inferred from timestamps alone.

Export queue selection can capture the current, primary, all, or chosen named
versions. Every task stores an independent immutable edit/metadata snapshot;
version names plus immutable IDs keep output suffixes distinct.

Merged derivatives now retain ordered many-source relationships with operation
names. Their badges and Source photos menu expose that provenance. Original
photos offer a Derived photos menu; external-editor derivatives offer a direct
Open original photo action. These links survive rename/relink through stable
catalog photo IDs.

Collections now form a hierarchy. Creating a collection lets you choose a
parent, and the context menu moves or removes the grouping. Cycles are refused.
Clicking a collection filters to it and its descendants; removing a collection
retains all photographs and promotes its children to the top level.

Search supports AND between words, OR between groups, quoted phrases, excluded
terms (`-word`), and `camera:`, `lens:`, `caption:`, `keyword:`, `type:`, `path:`,
`rating:>=4`, `iso:<=800`, and `flag:pick`/`reject`/`none`. The keyword browser
recognizes `Parent|Child` paths and counts matching photographs beneath each
ancestor. Hierarchical filters are saved with smart collections. XMP reads and
writes Lightroom's `lr:hierarchicalSubject` alongside flat keywords.

**Package selected photos** copies verified originals, authoritative local edits
and versions into a new movable project folder. A `.spproject` manifest stores
relative paths, content checksums, nested collections, stacks and available
source/derivative relationships. **Open portable project** verifies original
bytes before indexing the package in the current catalog, then restores those
relationships. Moving the whole package does not break its paths. Sidecars stay
editable; they are copied with verification but their subsequent edits do not
invalidate original-image checksums. Sources outside the selected package are
not copied or linked to missing local files.

## Practical limits

- Versions share one original's Library row and original thumbnail. Each can
  be opened and edited independently; they are not separate grid identities.
- Version removal while offline is refused so a stale remote version cannot
  reappear silently on reconnection.
- Import previews and capture dates come from decoded source metadata. A
  preview failure is displayed, and does not prevent a verified byte copy.
- Metadata templates apply to the imported primary edit. Preserved virtual
  versions retain their own metadata.
- Manual stacks are catalog organization, not pixel merges; the Photo Merge
  workflows create separate photographic derivatives.
- Stacks cannot be nested. Existing stacks must be dissolved before regrouping.
- Collection names remain globally unique for backwards compatibility.
- A portable project is a folder, not an archive. Keep the manifest and its
  `originals` subtree together. Opened photos stay in that folder.

## Verification

Regression tests cover immutable named-version identity after file moves,
offline version discovery, retained local edits on mirror failure, import subset
selection, safe date folders, template merge semantics, unchanged originals,
verified backups and sidecar retention, many-source provenance after rename,
and stack cover/membership preservation followed by file-preserving unstack.
Additional tests cover collection cycle refusal, field/OR/negative search,
hierarchical XMP interoperability, and reopening a moved portable project with
versions, stacks and nested collections followed by tampered-original refusal.
Checkpoint replay tests cover missing companions/backup, retained foreign edits,
and concurrent importer refusal. Annotation regression tests refuse malformed
or externally changed sidecars; gallery pick+rating changes are saved atomically.
