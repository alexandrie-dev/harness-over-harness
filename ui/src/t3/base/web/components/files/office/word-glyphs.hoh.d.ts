// Объявления web/components/files/office/word-glyphs.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { DisplayPage } from "@betteroffice/docx/layout/render";
/**
 * The page as the canvas paints it. The engine sets each glyph no bundled face has, such as Thai or Devanagari ones and
 * emoji, as a run of its own, which the canvas draws with the system's fonts. Neighbouring runs join here, so the
 * canvas shapes their text whole and marks join their letters. A run wider than the room the engine left it shrinks
 * into that room, rather than over the text after it.
 */
export declare function withWholeMissingGlyphs(page: DisplayPage, measure: OffscreenCanvasRenderingContext2D): DisplayPage;
