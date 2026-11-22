// Объявления client-runtime/device/duoSnap.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { Quaternion, Vector3 } from "three";
import type { DeviceScreenSize } from "./stream.ts";
export type DuoRestFace = "cover" | "inside" | "left" | "right";
export type DuoRestFrame = {
    face: DuoRestFace;
    normal: Vector3;
    up: Vector3;
    center: Vector3;
};
export type DuoViewSnap = {
    rotation: Quaternion;
    face: DuoRestFace;
    orientation: DeviceScreenSize["orientation"];
    center: Vector3;
    yawLimit: number;
};
export declare function duoViewSnaps(frames: readonly DuoRestFrame[], panel: 1 | 3): DuoViewSnap[];
export declare const nearestDuoView: (rotation: Quaternion, snaps: readonly DuoViewSnap[]) => DuoViewSnap | null;
