# Dynamic Develop sidebar

The Develop sidebar now switches between **Presets**, **AI** and **Workflow**.
The selector stays above the scrolling content, and each view retains its own
scroll position. This replaces the preset-only rail; the Library folder and
collection rail remains separate.

- Presets keeps the existing photo thumbnails, hover previews, Amount control,
  user presets and import/export actions. Hidden presets no longer generate
  GPU thumbnail renders, and switching views clears their transient preview.
- AI brings together Auto Balance/Auto Finish, AI RAW denoise,
  text/people/sky/point/depth selections, mask import, runtime setup and upscale
  model selection. Point and depth controls expand within the sidebar. Selection
  controls are shared with Tools → AI masks rather than separate handlers.
- Workflow exposes saved photo workflows, history/versions, photo merge, lens
  profiles, export/queue, print/contact sheets, galleries and workspace tools.
  Actions open existing tools and dialogs; sidebar navigation does not edit a
  photograph or create an undo step.

The selected view persists across restarts and is included in saved workspaces.
Older preferences and workspaces default to Presets. The selector is keyboard
focusable through normal egui Tab/arrow/activation behavior. English and Dutch
labels are included, with wrapped actions at the existing 236-pixel rail width.

Photo actions are disabled when no photo is open or while decoding, enhancement,
camera denoise, Auto Balance/Auto Finish or export is in flight. Cancel/status
controls remain accessible. AI RAW denoise requires decoded Bayer RAW data
and an installed model; selecting a mode creates one undoable
edit through the normal application path. Runtime/workspace/queue dialogs remain
accessible independently of photo readiness.

Validation includes actual egui pointer interactions for switching views,
loading-state gating, workflow export and camera-denoise undo. Preference tests
cover old workspace loading and persisted selection. The existing offscreen
WGPU acceptance harness now captures all three sidebar views in English and
Dutch alongside the existing dialogs. This is layout/interaction coverage,
not a claim of full native-window or assistive-technology acceptance.

The [0.4.2 personal macOS package](release-0.4.2.md) includes this sidebar.
