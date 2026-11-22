// Объявления client-runtime/device/phoneViewer.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type DeviceModelSource, type DeviceAccessorySource } from "./model.ts";
import type { DeviceScreenSize } from "./stream.ts";
import { type DeviceShapeProfile } from "./shapeProfile.ts";
export interface PhoneViewer {
    readonly setModel: (source: DeviceModelSource | null) => void;
    readonly setAccessory: (source: DeviceAccessorySource | null) => void;
    readonly frameUpdated: () => void;
    readonly setScreen: (screen: DeviceScreenSize | null, profile?: DeviceShapeProfile) => void;
    readonly setFoldAngle: (angle: number | null) => void;
    readonly resize: (width: number, height: number, pixelRatio: number) => void;
    readonly screenPoint: (x: number, y: number, captured?: boolean) => {
        x: number;
        y: number;
    } | null;
    readonly orbit: (deltaX: number, deltaY: number) => void;
    readonly setInteractionActive: (active: boolean, mode: "touch" | "orbit") => void;
    readonly resetPose: () => void;
    readonly dispose: () => void;
}
/** Owns only presentation resources. The caller retains the decoded canvas and the stream connection. */
export declare function createPhoneViewer(options: {
    readonly canvas: HTMLCanvasElement;
    readonly source: HTMLCanvasElement;
    readonly onUnavailable: () => void;
    readonly onModelError?: (cause: unknown) => void;
    readonly onFramingAspect?: (aspect: number) => void;
    readonly profile?: DeviceShapeProfile;
    readonly model?: DeviceModelSource | null;
    readonly accessory?: DeviceAccessorySource | null;
    readonly foldAngle?: number | null;
}): PhoneViewer;
