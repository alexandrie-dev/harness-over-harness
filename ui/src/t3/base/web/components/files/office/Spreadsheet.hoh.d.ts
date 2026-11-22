// Объявления web/components/files/office/Spreadsheet.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { WorkbookView } from "../officePreviewProtocol";
export default function Spreadsheet(props: {
    bytes: ArrayBuffer;
    view: WorkbookView;
    onDetails: (text: string) => void;
    onError: (cause: unknown) => void;
}): import("react").JSX.Element;
