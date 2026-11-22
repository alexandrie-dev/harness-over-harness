// Объявления web/components/desktop/SnapShotCoordinator.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type DesktopBridge, type DesktopPendingSnapShot, type ScopedThreadRef } from "@t3tools/contracts";
import { type DraftId } from "../../composerDraftStore";
type CaptureTarget = DraftId | ScopedThreadRef;
export declare function resolveExistingSnapShotTarget(target: CaptureTarget, routeThreadRef: ScopedThreadRef | null): CaptureTarget | null;
export declare function resolveSnapShotAttachmentTarget(pins: Map<string, DraftId | null>, id: string, target: CaptureTarget): CaptureTarget;
export declare function beginSnapShotAnimationWhenReady(id: string, target: Promise<CaptureTarget | null>, pendingStarts: Set<string>): Promise<void>;
export declare function dismissFailedSnapShot(id: string | undefined, soundedIds: Set<string>, pendingStarts: Set<string>): void;
export declare function resolveSnapShotDeliveryTarget(targets: Map<string, Promise<CaptureTarget | null>>, id: string, resolveTarget: () => Promise<CaptureTarget | null>): Promise<CaptureTarget | null>;
export declare function deliverSnapShot(bridge: DesktopBridge, item: DesktopPendingSnapShot, resolveTarget: () => CaptureTarget): Promise<void>;
export declare function SnapShotCoordinator(): null;
export {};
