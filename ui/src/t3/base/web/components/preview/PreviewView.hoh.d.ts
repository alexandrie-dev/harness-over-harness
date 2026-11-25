// Объявления web/components/preview/PreviewView.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type PreviewAnnotationPayload, type ScopedThreadRef } from "@t3tools/contracts";
import { type ComposerImageAttachment } from "~/composerDraftStore";
interface Props {
    threadRef: ScopedThreadRef;
    tabId?: string | null;
    configuredUrls?: ReadonlyArray<string> | undefined;
    visible: boolean;
    onSendAnnotation?: (annotation: PreviewAnnotationPayload, image: ComposerImageAttachment | null) => void;
}
export declare function PreviewView({ threadRef, tabId: requestedTabId, configuredUrls, visible, onSendAnnotation, }: Props): import("react").JSX.Element;
export {};
