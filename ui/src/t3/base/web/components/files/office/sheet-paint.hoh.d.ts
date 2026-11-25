// Объявления web/components/files/office/sheet-paint.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type CellRange, type GridMeta } from "@betteroffice/xlsx/headless";
import type { SheetFrame, SheetFrameRequest } from "./sheet-protocol";
/** The height of the column headers, in CSS pixels. */
export declare const headerHeight = 20;
/** The sizes of the header strips, in CSS pixels: the row headers' width and the column headers' height. */
export type Headers = {
    readonly width: number;
    readonly height: number;
};
/**
 * The frozen panes' size in a frame: the offset of the first track past them. Without panes, the first track can start
 * before the frame, part of it scrolled away.
 */
export declare function frozenSize(grid: GridMeta | undefined, frozen: {
    readonly rows: number;
    readonly cols: number;
}): {
    width: number;
    height: number;
};
/** A frame from the workbook worker and the request it answers, which says where its cells sit. */
export type Painted = {
    readonly request: SheetFrameRequest;
    readonly frame: SheetFrame;
};
/**
 * Draws a frame's cells beside the headers at the current scroll position. A frame painted at another position, as one
 * is while the next renders, moves with the scroll: the frozen panes stay put and the cells past them shift, leaving
 * the paper where the frame has no cells yet.
 */
export declare function paintCells(context: CanvasRenderingContext2D, painted: Painted, input: {
    readonly ratio: number;
    readonly headers: Headers;
    readonly x: number;
    readonly y: number;
    readonly width: number;
    readonly height: number;
}): void;
/**
 * A frame's grid at another scroll position, moved as `paintCells` moves its cells: the tracks past the frozen panes
 * shift by `shift`, and never under the panes, so headers, selection and hit tests follow the cells on screen.
 */
export declare function shiftGrid(grid: GridMeta, frozen: {
    readonly rows: number;
    readonly cols: number;
}, panes: {
    readonly width: number;
    readonly height: number;
}, shift: {
    readonly x: number;
    readonly y: number;
}): GridMeta;
/**
 * Draws the column letters and row numbers of a frame's tracks, frozen panes first, along the top and left edges.
 * Hidden tracks have no width in the grid, so they get no header.
 */
export declare function paintHeaders(context: CanvasRenderingContext2D, input: {
    readonly grid: GridMeta;
    readonly ratio: number;
    readonly headers: Headers;
    readonly width: number;
    readonly height: number;
    readonly selected: CellRange | undefined;
}): void;
/** Outlines the selected range and tints it, leaving its active cell clear, in the cells beside the headers. */
export declare function paintSelection(context: CanvasRenderingContext2D, input: {
    readonly grid: GridMeta;
    readonly ratio: number;
    readonly headers: Headers;
    readonly width: number;
    readonly height: number;
    readonly range: CellRange;
    readonly active: CellRange;
}): void;
