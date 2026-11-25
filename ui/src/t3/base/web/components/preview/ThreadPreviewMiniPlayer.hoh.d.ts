// Объявления web/components/preview/ThreadPreviewMiniPlayer.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type ScopedThreadRef } from "@t3tools/contracts";
import type { ChatCanvasPreview } from "../chat/chatCanvasLayout";
import { type PreviewMiniPlayerState } from "~/previewMiniPlayerStore";
interface Props {
    readonly threadRef: ScopedThreadRef;
    readonly miniPlayer: PreviewMiniPlayerState;
}
/** The mini player as the chat canvas lays it out; null when no player renders. */
export declare function useThreadPreviewCanvasSource(threadRef: ScopedThreadRef | null, miniPlayer: PreviewMiniPlayerState | null): ChatCanvasPreview | null;
export declare function ThreadPreviewMiniPlayer({ threadRef, miniPlayer }: Props): import("react").JSX.Element;
export {};
