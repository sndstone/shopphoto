# Optical lens profiles

Tools → Lens profiles and calibration manages local `.splens` JSON and its
`.splensdata` grid. Import validates the pair and assigns a unique name; export
copies a complete verified pair to a chosen folder. Disable preserves a copy
under the user lens directory's `disabled` folder for later re-import. Existing
profile files are never overwritten by these operations.

The calibration action accepts 1–64 evenly illuminated, defocused RAW flats
from one camera, lens and focal length. It measures each physical Bayer slot
relative to a central patch, excludes masked sensor borders, rejects clipped,
dark or extreme samples, averages repeated apertures and stores interpolatable
aperture grids. Exposure and per-channel illumination cancel against the center.
Distortion geometry is fitted from traced straight lines in the Lens panel and
can be saved into an installed profile.

A flat-field measurement cannot distinguish optical shading from uneven
illumination, dust or the photographed subject. Verify the result with a second
uniform capture. Camera/lens/focal/aperture metadata must be present. These are
user measurements, not a shipped proprietary correction database. Sensor dark,
bias and pixel-flat calibration for astrophotography is a separate workflow.

Tests cover synthetic radial falloff, clipped/dark refusal, and two-file profile
publication without replacement. Native RAW flat capture quality still requires
camera-specific validation; synthetic fixtures establish the measurement logic.
