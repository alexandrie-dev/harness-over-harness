// Объявления web/components/files/office/sheet-cells.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { XlsxAnchor } from "@betteroffice/xlsx";
import { type CellAddr, type CellRange, type Selection } from "@betteroffice/xlsx/headless";
/** The sheet's last cell, zero-based: row 1,048,576 in column XFD. */
export declare const lastCell: {
    row: number;
    col: number;
};
/** A column's letters, such as "A", "Z" or "AA". */
export declare function columnName(index: number): string;
/** The cells an export anchor names: one range for a cell or range anchor, none for any other kind. */
export declare function anchorRange(anchor: XlsxAnchor): CellRange[];
/** An A1 cell or range, such as "B2" or "A1:F20"; undefined for anything else. */
export declare function parseRange(a1: string): CellRange | undefined;
/** Grows a range until it covers every merged cell it touches, as Excel selects. */
export declare function coverMerges(range: CellRange, merges: readonly CellRange[]): CellRange;
/**
 * Moves a collapsed selection the way Excel moves its active cell around merged cells: landing in one selects its
 * first cell, and a step that stays inside the merged cell it started from steps past it.
 */
export declare function settleMerges(previous: Selection, next: Selection, merges: readonly CellRange[]): Selection;
/** A run of hidden rows or columns, zero-based and inclusive. */
export type Span = {
    readonly start: number;
    readonly end: number;
};
/** Hidden row spans such as "5:7", or column spans such as "C:D", as the export lists them. */
export declare function parseSpans(spans: readonly string[], axis: "rows" | "columns"): Span[];
/**
 * Moves a selection's focus off hidden rows and columns, on in the direction it moved, as Excel's arrow keys skip
 * them. A move that would leave the sheet stays where it started.
 */
export declare function skipHidden(from: CellAddr, selection: Selection, hidden: {
    readonly rows: readonly Span[];
    readonly columns: readonly Span[];
}): Selection;
/** Runs an engine call that throws on input it cannot take, such as a viewport with too many cells to draw. */
export declare function attempt<T>(run: () => T): T | undefined;
/** Whether a range holds a cell. */
export declare function contains(range: CellRange, cell: CellAddr): boolean;
