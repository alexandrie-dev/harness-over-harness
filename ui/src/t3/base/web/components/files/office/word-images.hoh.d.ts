// Объявления web/components/files/office/word-images.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { YrsMediaSource } from "@betteroffice/docx/yrs";
import type { VectorBitmap, VectorRequest } from "./vector-pictures";
type Size = {
    readonly width: number;
    readonly height: number;
};
/**
 * Resolves the display list's images in a worker: `media:{n}` sources decode from the session's bytes, `data:` and
 * `blob:` ones in place, and nothing else, so painting never makes a request. Undecodable parts paint a grey box. SVG
 * pictures, which the engine converts WMF and EMF ones to, need the window: `vectors` lists them, `provide` takes them.
 */
export declare function createBitmapImageResolver(media: (token: string) => YrsMediaSource | null): {
    resolve: any;
    /**
     * The SVG pictures the paints since the last call drew without a bitmap, or with one smaller than `sizes` asks, at
     * the size to rasterize them; a picture absent from `sizes` rasterizes at its own size.
     */
    vectors(sizes: ReadonlyMap<string, Size>): VectorRequest[];
    /** Keeps the window's rasterized SVG pictures in place of coarser ones. */
    provide(bitmaps: readonly VectorBitmap[]): void;
    /** Closes the least recently drawn images past the budget. Call it between paints, never during one. */
    trim(): void;
};
export {};
