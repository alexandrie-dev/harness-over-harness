// Объявления web/components/files/AttachmentFilePreview.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { AssetResource, EnvironmentId, ScopedThreadRef } from "@t3tools/contracts";
export declare function ReadOnlySourcePreview(props: {
    name: string;
    text: string;
    wordWrap?: boolean;
}): import("react").JSX.Element;
export declare function AttachmentFilePreview(props: {
    name: string;
    mimeType: string;
    sizeBytes: number;
    file?: Blob | null;
    asset?: {
        environmentId: EnvironmentId;
        attachmentId: string;
    } | {
        environmentId: EnvironmentId;
        resource: Extract<AssetResource, {
            _tag: "media-file" | "draft-workspace-file";
        }>;
    };
    htmlRender?: boolean;
    threadRef?: ScopedThreadRef;
    origin?: string;
    onRemove?: () => void;
    onClose?: () => void;
}): import("react").JSX.Element;
