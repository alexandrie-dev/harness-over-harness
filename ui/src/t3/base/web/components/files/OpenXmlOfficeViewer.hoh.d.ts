// Объявления web/components/files/OpenXmlOfficeViewer.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { OfficePreviewKind } from "@t3tools/shared/filePreview";
export default function OfficeViewer(props: {
    readonly src: string;
    readonly name: string;
    readonly kind: Extract<OfficePreviewKind, "docx" | "xlsx" | "pptx">;
    readonly sizeBytes?: number;
}): import("react").JSX.Element;
