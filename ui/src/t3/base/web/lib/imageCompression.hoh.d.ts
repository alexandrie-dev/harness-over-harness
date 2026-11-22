// Объявления web/lib/imageCompression.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
export declare const MAX_STASH_IMAGE_DATA_URL_CHARS = 1300000;
/** Bound source decoding: enormous ImageBitmaps can OOM the tab before resizing. */
export declare const MAX_COMPRESSIBLE_SOURCE_BYTES: number;
type ImageSize = {
    width: number;
    height: number;
};
export interface CompressedStashImage {
    imageSize?: ImageSize;
    dataUrl: string;
    mimeType: string;
    sizeBytes: number;
    recompressed: boolean;
}
export type ImageCompressionFailureReason = "too-large" | "unreadable";
export type CompressStashImageResult = {
    ok: true;
    image: CompressedStashImage;
} | {
    ok: false;
    reason: ImageCompressionFailureReason;
};
export type CompressImageFileResult = {
    ok: true;
    file: File;
    recompressed: boolean;
    imageSize?: ImageSize;
} | {
    ok: false;
    reason: ImageCompressionFailureReason;
};
/** Finder and some browsers omit the MIME type when dragging HEIC photos. */
export declare function isHeicImageFile(file: Pick<File, "name" | "type">): boolean;
export declare function readFileAsDataUrl(file: Blob): Promise<string>;
export declare function dataUrlToFile(dataUrl: string, name: string, mimeType: string): File;
/** `undefined` until the thumbnail settles, so a tile can paint a known thumbnail immediately. */
export declare function readComposerImageThumbnail(file: File): string | null | undefined;
export declare function createComposerImageThumbnail(file: File): Promise<string | null>;
/**
 * Pasted images are re-encoded after their placeholder already shows the source's thumbnail.
 * Reusing it keeps the tile from flashing back to its fallback while the new file is decoded.
 * Resolves once the target's thumbnail is readable, so a tile swapped to it paints at once even
 * when compression finished before the source's thumbnail did.
 */
export declare function inheritComposerImageThumbnail(source: File, target: File): Promise<void>;
export declare function compressImageForStash(file: File, budgetChars?: number): Promise<CompressStashImageResult>;
export declare function compressImageToByteLimit(file: File, maxBytes: number, options?: {
    preferredMimeType?: "image/jpeg";
    sourceSizeBytes?: number;
}): Promise<CompressImageFileResult>;
export declare function prepareImageForAttachment(file: File, maxBytes: number): Promise<CompressImageFileResult>;
export {};
