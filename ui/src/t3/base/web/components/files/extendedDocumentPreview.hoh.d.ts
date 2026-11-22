// Объявления web/components/files/extendedDocumentPreview.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type JSZip from "jszip";
import { type OfficeWorkerResult } from "./officePreviewProtocol";
export declare function openDocument(archive: JSZip, kind: "odt" | "ods" | "odp"): Promise<OfficeWorkerResult>;
export declare function openEpub(archive: JSZip): Promise<OfficeWorkerResult>;
export declare function openNotebook(bytes: ArrayBuffer): OfficeWorkerResult;
export declare function openRtf(bytes: ArrayBuffer): OfficeWorkerResult;
