// Объявления web/components/files/office/sheet-viewport.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { SheetViewport } from "./sheet-protocol";
export declare function cachedViewport(frame: SheetViewport, viewport: SheetViewport): {
    x: number;
    y: number;
};
export declare function bufferedViewport(viewport: SheetViewport): SheetViewport;
export declare function retainCachedViewport(cached: SheetViewport, incoming: SheetViewport, current: SheetViewport): boolean;
