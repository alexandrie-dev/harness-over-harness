// Объявления client-runtime/device/duoScene.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { Group, type PerspectiveCamera, type Texture } from "three";
import type { DuoRestFrame } from "./duoSnap.ts";
import type { DeviceScreenSize } from "./stream.ts";
export type DuoPanelId = 1 | 3;
export type DuoHingeLeaf = "left" | "right";
export type DuoFrameLayout = {
    width: number;
    height: number;
};
export declare function duoRawPoint(panel: DuoPanelId, x: number, y: number): {
    x: number;
    y: number;
};
export declare function duoFrameMatches(frame: DuoFrameLayout, screen: DeviceScreenSize): boolean;
export declare function duoDisplayKey(screen: DeviceScreenSize | null): string;
/** Asset resources belong to its model slot; this scene owns only live-screen replacement materials. */
export declare function createDuoScene(asset: Group, textures: Record<DuoPanelId, Texture>): {
    root: any;
    setAngle(angle: number): void;
    restFrames(panel: DuoPanelId): DuoRestFrame[];
    leafRotation(leaf: DuoHingeLeaf): any;
    hingeLeafAt(x: number, y: number, camera: PerspectiveCamera): DuoHingeLeaf | null;
    cancelInput(): void;
    screenPoint(x: number, y: number, camera: PerspectiveCamera, screen: DeviceScreenSize | null, readyKey: string, extend?: boolean): {
        x: number;
        y: number;
    } | null;
    dispose(): void;
};
