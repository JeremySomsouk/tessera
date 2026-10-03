# Presentation banner

`tessera-banner.png` — 2172 × 724, RGB PNG, 3:1 aspect ratio.

Used at the top of the repository README. The same asset is ready for the first Tessera project card on the personal website: use a wide container, `object-fit: cover`, and centered positioning. Keep the project title accessible outside the image as well. The mosaic and name are centered to tolerate narrower crops. Website placement is not changed by this repository PR.

Generated with the built-in ImageGen tool. Design prompt:

> A reusable wide GitHub README banner and portfolio project card for Tessera, a Rust desktop terminal gathering coding sessions in an Overview. A restrained editorial graphic, deep charcoal background, moss/sea-glass green mosaic of offset terminal-pane rectangles and tiny amber attention accents. Flat geometry, slight print grain, carefully controlled negative space. Large exact word “Tessera” and sparse `>_` motifs inside a central crop-safe area. Abstract brand artwork rather than a screenshot. No extra text, robots, laptop mockup, neon, purple gradient, watermark, or random code.


# Application icon

`app-icon.png` — 1024 × 1024 RGBA PNG, selected **Mosaic** concept. Four offset sea-glass/amber tesserae on a charcoal rounded square, with genuine transparency outside the icon. `app-icon-window.png` is its 256 × 256 export for the native viewport. These use the same artwork; no extra runtime dependencies are needed.

The macOS bundle script derives the standard 16/32/128/256/512 pixel icon family at 1×/2× using `sips`, builds `Contents/Resources/Tessera.icns` with `iconutil`, and declares it through `CFBundleIconFile`. Bundle validation decodes the ICNS back to an iconset before signing. Both Intel and Apple Silicon builds use these same assets.

Created with the built-in ImageGen tool from the selected Mosaic proposal, then resized only for packaging. Final design prompt:

> Faithfully reproduce proposal 1, Mosaic, as one standalone macOS icon. Preserve four offset tiles: upper-left larger sea-glass tile, upper-right smaller amber tile, lower-left smaller sea-glass tile, lower-right larger sea-glass tile, and clean negative-space channels. Keep the charcoal rounded-square backing, muted greens/amber, crisp edges and subtle material depth. Center on a square canvas with transparent margins; no external shadow, labels, numbers, extra symbols, or other proposals.
