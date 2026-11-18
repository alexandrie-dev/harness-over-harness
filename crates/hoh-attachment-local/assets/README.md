# Default CMYK input profile

`cmyk-input.icc.base64` contains the forward transforms of the **Chemical proof**
profile built into libvips 8.18.7, distributed with `@img/sharp-libvips-linux-x64`
1.3.4 and used by Sharp 0.35.5. The source archive and library were already in the
local npm cache; nothing was fetched or executed.

The profile's copyright tag explicitly releases the data into the public domain.
Its exact text is copied in `CMYK_PROFILE_LICENSE.txt`. No libvips implementation
or library binary is included in this crate.

The full original profile has 961,644 bytes and SHA-256
`156e7c14f244cfc4ed83a755ca4803d80e15dd249b40fae82cb127d3902e15c7`.
The stored input profile retains the original header and byte-identical `desc`,
`cprt`, `wtpt`, `bkpt`, `A2B0`, `A2B1`, and `A2B2` tag payloads. The three forward
tags reference the same original 53,754-byte LUT. Unused reverse transforms,
gamut table and large characterization text are omitted; offsets, tag count and
profile length are rebuilt, and the old profile ID is cleared. This does not
alter any CMYK-to-PCS transform. The resulting profile has 54,224 bytes and SHA-256
`92d1e4c5b1072b0e5b7c575c33a1cd36ef6ef311ce40ad917c01b110037a3ffe`.
Base64 is only the source-file representation; moxcms receives the ICC bytes.

The relevant Sharp 0.35.5 source is `src/pipeline.cc:374–397`:
embedded profiles use `icc_transform` with `embedded=true` and perceptual intent;
CMYK without a profile uses `input_profile="cmyk"`, also perceptual. See
<https://github.com/lovell/sharp/blob/v0.35.5/src/pipeline.cc#L374-L397>.
The attachment package always requests sRGB in `src/normalization.ts:80–83`.

`tests/cmyk.rs` uses generated constant-channel JPEGs and two independent colour
references. Its embedded synthetic ICC profile defines linear RGB as
`(1 - [C,M,Y]) * (1 - K)` before standard sRGB encoding. For the unprofiled case,
golden pixels are calculated from the original Chemical proof LUT's corner Lab
values using ICC v2 Lab scaling, D50-to-D65 Bradford adaptation, the standard
sRGB matrix and transfer function:

| CMYK ink bytes | Lab from the source LUT | Expected sRGB bytes |
|---|---|---|
| 0, 0, 0, 0 | 100, 0, 0 | 255, 255, 255 |
| 255, 0, 0, 0 | 56.6536, -36.7821, -52.9712 | 0, 156, 228 |
| 0, 255, 0, 0 | 52.2110, 77.5685, -3.0351 | 234, 23, 133 |
| 0, 0, 255, 0 | 94.9259, -3.8526, 101.1208 | 255, 240, 0 |
| 0, 0, 0, 255 | 14.6805, 3.9796, -4.2452 | 41, 35, 43 |
| 255, 255, 255, 255 | 13.1603, 2.0015, -7.1400 | 33, 33, 44 |

Each RGB channel allows an absolute error of 3 levels out of 255 before encoding,
and 6 after the quality-85 JPEG round trip. These tolerances cover CMS and JPEG
rounding; they do not admit replacing the fallback profile with naive CMYK
multiplication. Every pixel is checked, including the second JPEG MCU.
