// Объявления client-runtime/device/phoneScene.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { Group, Mesh, type Camera, type BufferGeometry, type Texture } from "three";
import type { DeviceScreenSize } from "./stream.ts";
import { type DeviceShapeProfile } from "./shapeProfile.ts";
export declare function phoneDisplayLayout(screen: DeviceScreenSize | null, rawWidth: number, rawHeight: number): {
    aspect: number;
    rotation: number;
    landscape: boolean;
    rawLandscape: boolean;
};
export declare function createPhoneScene(texture: Texture, layout: ReturnType<typeof phoneDisplayLayout>, profile?: DeviceShapeProfile): {
    root: any;
    orientation: any;
    width: number;
    height: number;
    screenPoint: (x: number, y: number, camera: Camera, captured?: boolean) => {
        x: number;
        y: number;
    } | null;
    setDisplay(nextTexture: Texture, nextLayout: PhoneDisplayLayout): void;
    dispose(): void;
};
export type PhoneDisplayLayout = ReturnType<typeof phoneDisplayLayout>;
export declare function updateDisplayUv(geometry: BufferGeometry, width: number, height: number, layout: PhoneDisplayLayout): void;
export declare function createDisplayProjection(display: Mesh, orientation: Group, width: number, height: number, getLayout: () => PhoneDisplayLayout): (x: number, y: number, camera: Camera, captured?: boolean) => {
    x: number;
    y: number;
} | null;
