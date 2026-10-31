# Tethered capture

Open **Tether capture**, choose a destination, and run **Detect backend and cameras**. ShopPhoto runs the locally installed `gphoto2` executable (or `SHOPPHOTO_GPHOTO2`) and inspects detected ports and each driver's reported still-image capture ability. Capture stays disabled without that specific capability. The application does not install a backend, bundle a proprietary SDK, infer support from a camera brand, or change exposure settings.

**Capture one image** uses one `--capture-image-and-download --keep` invocation, with separate process arguments and an explicit detected port/model. The `--keep` option asks gphoto2 to retain the camera-side file; actual camera storage behavior depends on the driver and capture target. A temporary directory outside the watched destination receives downloads; completed files are then synced and published atomically without overwriting existing files. The destination is scanned into Library and a supported result opens for editing. RAW+JPEG downloads are both kept. Unsupported formats are saved and reported.

Read-only backend/device detection has timeouts. Capture has a three-minute timeout and cancellation kills/reaps the backend process; cancelling cannot reverse an already triggered shutter. Output diagnostics are bounded. Exposure, live view, bulb sequencing and remote camera configuration are not provided by this adapter. Close another camera-control application before direct capture.

For Hasselblad Phocus or another capture application, select its output directory and choose **Watch folder from Phocus / other capture app**. This adds the folder through the existing catalog scan/watch path. Phocus controls the camera; ShopPhoto imports the resulting supported files. Native camera control is not claimed for Phocus.

The process follows the official [gphoto2 CLI capture and retention options](https://gphoto.github.io/doc/manual/ref-gphoto2-cli.html); capability parsing follows gphoto2's [camera-abilities output implementation](https://github.com/gphoto/gphoto2/blob/master/gphoto2/actions.c).

Validation covers capability parsing and a real fixture executable that detects a device, requires `--keep`, writes a download, and verifies complete-file publication. The development machine has no installed `gphoto2` backend or attached capture camera; physical capture and operating-system device permissions have not been tested.
