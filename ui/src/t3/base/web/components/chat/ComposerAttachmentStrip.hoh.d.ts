// Объявления web/components/chat/ComposerAttachmentStrip.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { ChatAttachment, ChatFileAttachment, EnvironmentId, ScopedThreadRef } from "@t3tools/contracts";
import { type ComposerFileAttachment, type ComposerImageAttachment, type DraftId } from "../../composerDraftStore";
import { type PendingSnapShotAnimation } from "../../lib/snapShotAnimation";
import type { PreparingComposerImage } from "./preparingComposerImages";
import { type ExpandedImagePreview } from "./ExpandedImagePreview";
/** Image, SnapShot and video tiles above the composer editor. */
export declare function ComposerAttachmentMediaRow({ images, expandedImages, preparingImages, videos, pendingSnapShotAnimations, pendingSnapShotIds, uncommittedSnapShotIds, nonPersistedImageIds, snapShotAnimations, supportsAttachmentUploads, maxFileAttachmentBytes, environmentId, attachmentDraftTarget, canOperateThread, onExpandImage, onCopySnapshot, onFileOpen, onRemoveImage, onRemoveFile, }: {
    readonly images: ReadonlyArray<ComposerImageAttachment>;
    readonly expandedImages: ReadonlyArray<ComposerImageAttachment>;
    readonly preparingImages: ReadonlyArray<PreparingComposerImage>;
    readonly videos: ReadonlyArray<ComposerFileAttachment>;
    readonly pendingSnapShotAnimations: ReadonlyArray<PendingSnapShotAnimation>;
    readonly pendingSnapShotIds: ReadonlyArray<string>;
    readonly uncommittedSnapShotIds: ReadonlyArray<string>;
    readonly nonPersistedImageIds: ReadonlyArray<string>;
    readonly snapShotAnimations: boolean;
    readonly supportsAttachmentUploads: boolean;
    readonly maxFileAttachmentBytes: number | null;
    readonly environmentId: EnvironmentId;
    readonly attachmentDraftTarget: ScopedThreadRef | DraftId;
    readonly canOperateThread: boolean;
    readonly onCopySnapshot: (imageId: string) => Promise<void>;
    readonly onExpandImage: (preview: ExpandedImagePreview) => void;
    readonly onFileOpen: (attachment: ChatFileAttachment) => void;
    readonly onRemoveImage: (imageId: string) => void;
    readonly onRemoveFile: (fileId: string) => void;
}): import("react").JSX.Element;
/** Non-media file attachments, listed under the media row. */
export declare function ComposerAttachmentFileList({ files, resolvedTheme, supportsAttachmentUploads, maxFileAttachmentBytes, fileStagingLimit, environmentId, attachmentDraftTarget, canOperateThread, onPreviewFile, onRemoveFile, }: {
    readonly files: ReadonlyArray<ComposerFileAttachment>;
    readonly resolvedTheme: "light" | "dark";
    readonly supportsAttachmentUploads: boolean;
    readonly maxFileAttachmentBytes: number | null;
    readonly fileStagingLimit: number | null;
    readonly environmentId: EnvironmentId;
    readonly attachmentDraftTarget: ScopedThreadRef | DraftId;
    readonly canOperateThread: boolean;
    readonly onPreviewFile: (fileId: string) => void;
    readonly onRemoveFile: (fileId: string) => void;
}): import("react").JSX.Element;
export declare function ComposerFilePreviewDialog({ file, onRemove, onPreviewFileIdChange, }: {
    readonly file: ComposerFileAttachment;
    readonly onRemove: (fileId: string) => void;
    readonly onPreviewFileIdChange: (fileId: string | null) => void;
}): import("react").JSX.Element;
/** Thumbnails beside the single-line prompt while the composer rests. */
export declare function ComposerRestingImagePreviews({ images, allImages, resolvedTheme, onExpandImage, onCopySnapshot, onShowMore, }: {
    readonly images: ReadonlyArray<ComposerImageAttachment>;
    readonly allImages: ReadonlyArray<ComposerImageAttachment>;
    readonly resolvedTheme: "light" | "dark";
    readonly onCopySnapshot: (imageId: string) => Promise<void>;
    readonly onExpandImage: (preview: ExpandedImagePreview) => void;
    readonly onShowMore: () => void;
}): import("react").JSX.Element;
/** Attachments of the queued message being edited; they are already uploaded. */
export declare function ComposerQueuedAttachmentRow({ attachments, onRemove, }: {
    readonly attachments: ReadonlyArray<{
        readonly attachment: ChatAttachment;
        readonly url: string | null;
    }>;
    readonly onRemove: (attachmentId: string) => void;
}): import("react").JSX.Element;
export declare function ComposerAttachButton({ addComposerAttachments, focusComposer, }: {
    readonly addComposerAttachments: (files: File[]) => Promise<boolean>;
    readonly focusComposer: () => void;
}): import("react").JSX.Element;
