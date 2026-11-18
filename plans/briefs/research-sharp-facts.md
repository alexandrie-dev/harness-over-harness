# Research: five facts about sharp and libvips, from their source

Working directory `/home/save/Alexandrie/HarnessOverHarness`. Install nothing, run no builds, change no file; your answer is your final message. Use the web to read the **source code** of the exact versions (GitHub: lovell/sharp, libvips/libvips, randy408/libspng, libjpeg-turbo); documentation and memory are not enough. Cite file, line range, tag or commit for every answer; where you could not read the source, say "not checked".

Versions: dsh uses `sharp` 0.35.5; the port's notes (`crates/hoh-attachment-local/PORT_NOTES.md`) name libvips 8.18.7 through sharp-libvips 1.3.4. Check the version pairing first.

Context: the Rust port `crates/hoh-attachment-local` of `/mnt/toshiba/alexandrie/hoh/refs/deepseek-harness/packages/attachment/attachment-local` decides whether an uploaded image is stored byte for byte or re-encoded (`src/image.ts:52-80`, `src/normalization.ts`: `canPassThroughNormalization`). A wrong decision changes the SHA-256 of the stored file and with it the attachment id written to the session log. A reviewer could not verify five behaviours of sharp:

1. `sharp(input).metadata()`: for a JPEG that contains a COM marker segment, is `comments` present (non-empty)? Which formats fill `comments` at all, and from which libvips metadata fields? (sharp `src/metadata.cc`, libvips `jpeg2vips.c` / `vipspng.c` / `spngload.c`.)
2. For an animated PNG (APNG, with `acTL`), what is `metadata().pages`? Does libvips load APNG frames at all in that version?
3. PNG text chunks (`tEXt`, `zTXt`, `iTXt`) located after `IDAT`, and chunks located after `IEND`: are they reported in `comments` by the PNG loader libvips uses in sharp's prebuilt binaries (libspng or libpng — establish which)?
4. A four-component JPEG: when does the loader invert the channels (Adobe APP14 marker present or absent, transform flag 0 or 2)? What does `toColourspace('srgb')` do for a CMYK image with no embedded profile, with an embedded CMYK profile, and with an embedded profile that is not CMYK (sharp `src/pipeline.cc`, the built-in `cmyk` profile)? Is libvips' built-in `cmyk` profile the "Chemical proof" profile by Graeme W. Gill, and under what licence is it distributed in the libvips repository?
5. `resize({ width, height, fit: 'inside', withoutEnlargement: true })`: how are the output dimensions computed and rounded (sharp `src/pipeline.cc` / `lib/resize.js`)? Give the result for 4000×3 into a 2000×1 box and for 3000×2001 into 2000×2000.

For each fact: the answer in one or two sentences, the source lines, and what the Rust port should therefore do (the port's current behaviour is in `crates/hoh-attachment-local/src/image.rs:150-200, 340-420`, read it). At most 900 words.
