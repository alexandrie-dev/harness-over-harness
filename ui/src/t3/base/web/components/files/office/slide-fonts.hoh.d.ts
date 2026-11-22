// Объявления web/components/files/office/slide-fonts.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { PresentationHandle } from "@betteroffice/pptx";
import type { DeckText } from "./slide-text";
/**
 * The faces one open deck draws with, added to `fonts`: the font set of the deck's own worker, which paints it. The
 * engine shapes each face under the family name the deck uses, and the browser holds the same bytes under a private
 * name that the laid-out frames are renamed to. Painting then matches shaping, and the deck's names (such as Inter or
 * Segoe UI) never replace a font the painter itself needs.
 */
export declare function slideFonts(deck: PresentationHandle, fonts: FontFaceSet): {
    /**
     * Registers the faces the deck's text names, in the order `text` ranks them: the engine draws every family it
     * cannot find with the first. Resolves once each face has loaded or failed.
     */
    load: (text: DeckText) => Promise<void>;
    /**
     * Lays a slide out. When its text asks for a style a registered face lacks, such as a bold the title style
     * inherits, that face loads and the slide lays out again. The frame names the browser's faces, ready to paint.
     */
    layout: (index: number) => Promise<any>;
};
