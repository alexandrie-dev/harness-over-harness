// Объявления web/lib/previewAnnotation.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { PreviewAnnotationPayload } from "@t3tools/contracts";
export type PreviewAnnotationCapture = {
    readonly status: "captured";
    readonly file: File;
} | {
    readonly status: "none";
} | {
    readonly status: "failed";
};
/** Decode Electron's PNG crop locally; fetching a data URL violates desktop connect-src. */
export declare function capturePreviewAnnotationScreenshot(annotation: PreviewAnnotationPayload): PreviewAnnotationCapture;
