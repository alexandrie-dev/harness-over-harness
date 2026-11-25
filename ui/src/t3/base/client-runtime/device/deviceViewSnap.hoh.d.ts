// Объявления client-runtime/device/deviceViewSnap.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { Quaternion } from "three";
export declare function nearestDeviceView<T extends {
    rotation: Quaternion;
    yawLimit: number;
}>(rotation: Quaternion, snaps: readonly T[]): T | null;
