// Объявления web/components/files/office/sheet-frame.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type DisplayList, type WorkbookHandle } from "@betteroffice/xlsx";
import type { SheetViewport } from "./sheet-protocol";
/** Part of a frame: its display list, where its origin sits in the frame, and where it starts to show. */
type Tile = {
    readonly list: DisplayList;
    readonly x: number;
    readonly y: number;
    readonly clipX: number;
    readonly clipY: number;
};
/**
 * A frame of cells: the tiles that draw it, one display list for its grid and links, without draw commands, and the
 * viewport it shows.
 */
export type Frame = {
    readonly list: DisplayList;
    readonly tiles: readonly Tile[];
    readonly viewport: SheetViewport;
};
/**
 * The cells of one viewport, as the engine draws them; `undefined` when even its smallest tiles fail. A `wider`
 * viewport around it is drawn instead when the engine takes it whole.
 */
export declare function drawFrame(workbook: WorkbookHandle, viewport: SheetViewport, frozen: {
    readonly rows: number;
    readonly cols: number;
}, wider?: SheetViewport): Frame | undefined;
/** A frame without the gridlines the engine draws on every sheet, for a sheet whose view hides them. */
export declare function withoutGridlines(frame: Frame): Frame;
/**
 * A frame whose chart text names real fonts. The engine leaves the theme's `+mj-lt` and `+mn-lt` in chart text and the
 * painter sets chart families as CSS as they are, so an unresolved or missing family would keep the previous font.
 */
export declare function chartFonts(frame: Frame, theme: {
    readonly major: string;
    readonly minor: string;
}): Frame;
/**
 * Draws a frame's tiles with the cells' top-left corner at `origin`, in device pixels, each tile from its seam on, so a
 * tile covers the panes it repeats.
 */
export declare function paintFrame(context: OffscreenCanvasRenderingContext2D, frame: Frame, input: {
    readonly ratio: number;
    readonly origin: {
        readonly x: number;
        readonly y: number;
    };
}): void;
export {};
