// Объявления client-runtime/device/androidFoldScene.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type Camera, type Texture } from "three";
import type { PhoneDisplayLayout } from "./phoneScene.ts";
export declare const DEFAULT_FOLD_INNER_ASPECT: number;
export declare const isFoldInnerAspect: (aspect: number) => boolean;
export declare function createAndroidFoldScene(texture: Texture, layout: PhoneDisplayLayout, initialAngle: number, innerAspect?: number): {
    root: any;
    orientation: any;
    width: number;
    height: number;
    innerAspect: number;
    setAngle: (next: number) => void;
    setDisplay: (nextTexture: Texture, nextLayout: PhoneDisplayLayout) => void;
    screenPoint(x: number, y: number, camera: Camera, captured?: boolean): {
        x: number;
        y: number;
    } | null;
    dispose(): void;
};
