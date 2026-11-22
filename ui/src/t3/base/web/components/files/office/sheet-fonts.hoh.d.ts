// Объявления web/components/files/office/sheet-fonts.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { DisplayList } from "@betteroffice/xlsx";
/**
 * The metric-compatible faces a workbook's cells draw with, such as Carlito for Calibri, added to the worker's font set
 * under the family name the painter asks for, so text widths, wrapping and overflow match Excel on machines without
 * Office fonts. A family the app ships no face for stays with the browser's own fallback.
 */
export declare function sheetFonts(): (lists: readonly DisplayList[]) => Promise<any[]>;
