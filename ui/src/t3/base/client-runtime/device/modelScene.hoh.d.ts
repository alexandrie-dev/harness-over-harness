// Объявления client-runtime/device/modelScene.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { Group, Texture, type Object3D } from "three";
import type { DeviceAssetSource } from "./model.ts";
import { type PhoneDisplayLayout } from "./phoneScene.ts";
/** GLB resources belong to one viewer; the live framebuffer texture belongs to its stream presentation. */
export declare function disposeDeviceModel(root: Object3D): void;
export declare function createImportedPhoneScene(asset: Group, texture: Texture, initial: PhoneDisplayLayout): {
    root: any;
    orientation: any;
    width: number;
    height: number;
    setDisplay(nextTexture: Texture, nextLayout: PhoneDisplayLayout): void;
    screenPoint: (x: number, y: number, camera: import("three").Camera, captured?: boolean) => {
        x: number;
        y: number;
    } | null;
    dispose(): void;
};
export declare function loadDeviceModel(source: DeviceAssetSource, signal: AbortSignal): Promise<{
    asset: any;
    dispose: () => void;
}>;
