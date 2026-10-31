# ShopPhoto 0.2.5 release audit

ShopPhoto 0.2.5 completes every locally actionable item in the post-0.2.3
review and keeps the release on the patch-version line.

## Final usability fixes

- UI scale is now adjusted as a stable 1% draft and changes the live layout
  only when Apply is pressed.
- Catalog scans never bulk-read RAW payloads on network filesystems merely to
  generate duplicate hashes. Existing unchanged hashes are reused.
- SMB shares do not receive recursive native filesystem watchers. They can be
  rescanned with the explicit Refresh folders action.
- Folder discovery, availability checks, thumbnail cache lookup and thumbnail
  decoding stay on workers rather than the egui thread.
- A failed or partial network traversal preserves existing catalog rows,
  continues loading other folders, and marks the unavailable folder in the UI.
- macOS network-mount detection reads the cached mount table and does not call
  `statfs` on a potentially stalled SMB path.

## Verification

- `cargo test --workspace --all-targets --locked`: 326 tests passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- Regression tests verify that a network scan does not read a new/changed RAW
  and that an inaccessible folder cannot erase cached library entries.
- Universal arm64/x86_64 release build, strict bundle signature validation and
  `hdiutil verify`: passed.
- The packaged app completed a real launch/screenshot smoke test without the
  previous multi-second main-thread thumbnail stall.
