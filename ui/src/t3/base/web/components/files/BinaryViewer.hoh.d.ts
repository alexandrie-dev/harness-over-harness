// Объявления web/components/files/BinaryViewer.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
export default function BinaryViewer(props: {
    readonly src: string;
    readonly name: string;
    readonly kind: "archive" | "sqlite";
    readonly onRetry?: () => void;
}): import("react").JSX.Element;
