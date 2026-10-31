# Catalog reliability and offline editing

The photographic workflow update adds non-destructive XMP merging, verified
imports, catalog maintenance, and full-resolution offline copies.

## Available workflows

- **Tools → Library & offline → Back up catalog** writes a consistent SQLite
  snapshot, including pending WAL data, and checks its integrity. Existing
  backup files are not overwritten. Originals and `.spedit`/XMP sidecars remain
  in their photo folders and need their own backup.
- **Check catalog integrity** checks SQLite integrity and foreign-key links.
- **Restore catalog from backup** validates a read-only source, migrates a
  private staging copy, and writes a rescue snapshot of the current catalog.
  SQLite's atomic backup transaction restores the database while keeping WAL
  readers valid. The app releases its persistence handle before restoring,
  clears old watchers/undo, and reloads the restored catalog. Source originals
  and edit sidecars are retained. Rescue snapshots can be restored the same way.
- **Relink moved folder** (also in each folder's context menu) verifies the
  replacement files before changing paths. Known content hashes must match.
  Photo IDs, collections and external-editor companion relationships survive.
- **Keep selected photos offline** creates verified full-resolution local
  copies. Pinned copies are exempt from the normal LRU budget. **Release selected
  offline copies** removes that exemption without deleting edits. The toolbar
  identifies available and currently used offline copies.
- **Reload metadata from XMP** imports the current ratings, caption, keywords,
  label, GPS, creator, copyright, usage terms and creator contact email. Image
  adjustments remain unchanged.

XMP saves retain foreign namespaces, camera-raw edit structures, comments and
unrecognized contact fields. Namespace URIs identify properties; a foreign
property named `Rating` is not mistaken for Adobe's rating. Malformed,
unsupported or oversized documents are refused rather than replaced. An open
photo keeps an XMP revision guard shared with the persistence worker; edits
made by another program since opening produce an actionable conflict message.
Queued ShopPhoto saves advance the same guard. Annotation-only Library/culling
updates preserve descriptive XMP fields.

Moving/renaming includes virtual-copy sidecars, checks all destinations before
moving anything, refuses collisions, and supports verified cross-volume copies.
The app updates existing catalog rows and remaps current/session selections;
Undo restores both filesystem paths and catalog relationships.

Card/folder import verifies copied content after flushing it, then makes any
backup from that verified copy. Both primary and backup destination collisions
are avoided. Hashing streams through bounded buffers. Import names are sanitized
so naming tokens cannot escape the selected destination.

## Practical limits

- Offline copies are complete originals, not small lossy smart previews. Their
  disk cost is the original file size; pinned data may exceed the LRU budget.
- Legacy cache entries are upgraded with a content digest when next opened
  online. Only verified copies can be opened while the original is unavailable.
- Relink uses stored hashes where available; new network files may not have
  hashes, in which case relative paths and existence provide the checks.
- XMP conflict detection is optimistic: it compares the saved revision again
  immediately before atomic publication. Uncooperative foreign programs do not
  participate in a cross-application transaction.
- Offline sidecar synchronization retains its existing retry-on-next-edit
  behavior. Failed synchronization leaves the local edit intact.

## Verification

Catalog regression tests cover foreign-XMP preservation and alias prefixes,
malformed XML refusal, stale revision rejection, contact-field preservation,
verified copies, backup collisions, virtual-copy move/undo, catalog relationship
preservation, backup integrity, relinking across path aliases, offline opening,
pinning/eviction and corrupt-cache rejection. A persistence-worker integration
test queues several saves through one revision guard, then verifies that an
external edit is preserved while ShopPhoto's own `.spedit` still saves.
Restore tests keep a second live reader open, verify rescue recovery, and
check that a corrupt backup cannot change the current catalog.
