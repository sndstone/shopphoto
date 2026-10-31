# ShopPhoto 0.4.7 — guided AI selections and cleaner layer edges

This release makes AI selection a guided, visible workflow and removes the
halos that layer clarity drew around selected subjects. It retains everything
from 0.4.6.

## Changes

- The AI rail starts with a live status card that says what is happening and
  what to do next. Choose a method (Subject, Describe, Click or Depth); only
  that method's numbered steps are shown. Created layers get a final "Check and
  edit the selection" step with a mask toggle and a jump to Local & retouch.
- Detected people and vehicles are tinted, outlined and numbered in the
  preview, colour-matched to their rail rows. Click an element in the preview
  to include or skip it; a banner offers "Create N adjustment layers".
- Mask overlays use a high-contrast pink instead of the blue accent, which was
  nearly invisible over skies and water. A banner names the shown mask.
- Point picking shows which point type is armed; keep/exclude points are solid,
  numbered pins.
- Presets / AI / Workflow, the selection methods and Camera AI denoise modes
  use a full-width segmented control matching Library / Develop.
- Layer clarity uses perceptual lightness with an edge- and coverage-weighted
  local mean. Bright or dark backgrounds no longer print rims or ghosts onto a
  selected subject (for example around fingers, sleeves and collars).

## Validation

sp-app (106 passed, 11 ignored) and sp-pipeline (all suites, including the
local-adjustment golden and tile-origin invariance tests) pass. Layer clarity
+80 with +1.5 EV on a two-person PNG was compared before and after: face
interior change stays visible (12.6 vs 18.0 mean, the latter including
artifacts) while the sleeve/hand edge change fell from 5.3 to 2.3 and the dark
rims disappeared. Rail, banner and overlay states were inspected in screenshots;
clicking elements in the preview was not exercised with real pointer input.

Global clarity, the fixed 33 px matting band on very large files and the
1536 px guided-refinement bake are unchanged and remain candidates for later
edge work.
