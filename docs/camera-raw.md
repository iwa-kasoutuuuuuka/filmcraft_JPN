# Camera raw (DNG)

FilmCraft imports Digital Negative (`.dng`) camera raw files as stills (File ▸ Import, the Media
Browser, image sequences such as CinemaDNG frame folders). DNG is the open, royalty-free raw format:
many cameras and phones write it natively, and other raw formats convert to it. Proprietary raw
formats (CR3, NEF, ARW, RAF…) are not read; convert them to DNG first.

The decoder is `crates/media/src/still/dng.rs` (lossless JPEG in `dng/ljpeg.rs`), written from the
public DNG specification, TIFF 6.0 and ITU-T T.81. It is pure Rust, runs on the web build too, and
treats every byte of the file as hostile (capped sizes and directory walks, errors instead of
panics; truncation and bit-flip tests).

## What it reads

- **Structure:** little- and big-endian TIFF, IFD chains and `SubIFDs` (loops and repeats walked
  once). The raw image is the largest directory with `NewSubFileType` 0 and photometric
  interpretation CFA or LinearRaw; a file is a DNG when IFD0 has `DNGVersion`.
- **Data:** strips or tiles; uncompressed 8–16 bits per sample (other depths bit-packed MSB first),
  lossless JPEG (all seven predictors, restart intervals, point transform, the usual
  two-components-per-row tile layout), Deflate (with predictor 2), lossy JPEG.
- **Levels:** `ActiveArea`, `LinearizationTable`, `BlackLevel` with `BlackLevelRepeatDim` and
  `BlackLevelDeltaH`/`V`, `WhiteLevel`.
- **Mosaic:** any 2×2 Bayer arrangement (`CFAPattern`, `CFAPlaneColor`), demosaiced with the
  Malvar–He–Cutler gradient-corrected linear filters. LinearRaw (already demosaiced RGB, or
  monochrome) needs none.
- **Colour:** white balance from `AsShotNeutral` (else `AsShotWhiteXY`, else D65); camera to XYZ
  from `ForwardMatrix`, or from `ColorMatrix` (with `CameraCalibration` and `AnalogBalance`) plus a
  Bradford adaptation; dual-illuminant matrices interpolated by the white's colour temperature;
  `BaselineExposure`; highlights clipped where the first channel saturates so they stay neutral;
  output is 8-bit sRGB.
- **Geometry:** `DefaultCropOrigin`/`Size`, then `Orientation`.

## What it does not do (yet)

- X-Trans and other non-2×2 colour filter patterns, non-rectangular CFA layouts, floating-point
  (HDR) DNGs and planar data are refused. Only in that case (an *unsupported* raw, never a corrupt
  one) the largest embedded RGB preview is shown instead, so the clip is usable but not the raw.
- Not applied: the camera profile's tone curve and hue/saturation maps (the picture is a plain
  linear-to-sRGB rendering, flatter than Lightroom's default), opcode lists (lens distortion,
  vignetting, bad pixels), `DefaultScale` (non-square pixels), noise reduction.
- No raw adjustments (exposure, white balance, highlight recovery) in the UI, and the still is 8-bit
  sRGB; a high-bit-depth path is future work.
- Pictures over 160 megapixels are refused (memory).
