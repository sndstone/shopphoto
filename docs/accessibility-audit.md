# Accessibility and keyboard audit — 0.2.3

ShopPhoto enables eframe's native AccessKit bridge, so buttons, checkboxes,
sliders, text fields, menus, dialogs and their enabled/selected states reach
VoiceOver, Narrator and Linux accessibility services. The central theme keeps
normal text above WCAG AA contrast (covered by `theme` tests), gives every
inactive control a boundary and every active/focused control an accent stroke,
and uses a 28-point minimum interaction height. UI scale is persisted from
75–200%.

Keyboard audit:

- Top bar, left rail, tool accordions, sliders, combo boxes, metadata fields,
  library filters and export/import dialogs use standard egui widgets and are
  reachable in Tab order. Space/Enter activates the focused control and Escape
  closes menus/dialogs.
- The image canvas keeps pointer gestures for brush strokes, crop handles and
  guide drawing, while every resulting edit remains adjustable from a
  keyboard-reachable panel control. Undo/redo, before/after, clipping, rating,
  labels, navigation, export and shortcut help have documented shortcuts.
- Destructive trash has a focused confirmation modal. Background operations
  expose text plus progress and a keyboard-reachable Cancel action.
- Custom canvas hit regions (crop corners, split divider and mask handles) are
  supplemental: they do not hide the corresponding sliders/toggles.

Localized strings load from `assets/i18n/*.json` through one English-fallback
table. English and Dutch catalogs are parity-tested, so adding another language
is a data-file change plus one enum entry rather than a widget refactor. New UI
copy should enter that table as panels are touched; untranslated copy safely
remains English.
