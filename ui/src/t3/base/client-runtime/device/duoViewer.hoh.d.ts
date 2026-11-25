// Объявления client-runtime/device/duoViewer.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type DuoPanelId } from "./duoScene.ts";
import { type DeviceModelSource } from "./model.ts";
import type { DeviceScreenSize } from "./stream.ts";
export interface DuoViewer {
    readonly setScreen: (screen: DeviceScreenSize | null) => void;
    readonly setHingePreview: (angle: number | null, pinch?: boolean) => void;
    readonly setInteractionActive: (active: boolean, mode?: "touch" | "orbit") => void;
    readonly rejectOrientation: () => void;
    readonly frameUpdated: (panel: DuoPanelId, primary?: HTMLCanvasElement) => void;
    readonly resize: (width: number, height: number, pixelRatio: number) => void;
    readonly screenPoint: (x: number, y: number, captured?: boolean) => {
        x: number;
        y: number;
    } | null;
    readonly orbit: (x: number, y: number) => void;
    readonly beginHinge: (x: number, y: number) => boolean;
    readonly resetPose: () => void;
    readonly cancelInput: () => void;
    readonly dispose: () => void;
}
export declare function createDuoViewer(options: {
    canvas: HTMLCanvasElement;
    sources: Record<DuoPanelId, HTMLCanvasElement>;
    model: DeviceModelSource;
    onUnavailable: () => void;
    onModelError?: (cause: unknown) => void;
    onPanelRequested?: (panel: DuoPanelId) => void;
    onOrientationRequested?: (orientation: DeviceScreenSize["orientation"]) => void;
}): DuoViewer;
