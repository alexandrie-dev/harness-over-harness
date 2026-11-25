// Объявления client-runtime/device/deviceMotion.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { Quaternion } from "three";
export declare function createDeviceMotion(options: {
    choose: (rotation: Quaternion) => Quaternion;
}): {
    rotation: any;
    setPose(next: Quaternion, now: number, immediate?: boolean): void;
    dragActive(active: boolean, now: number): void;
    orbit(x: number, y: number, now: number): void;
    hold(active: boolean, now: number): void;
    advance: (now: number, reduced?: boolean) => boolean;
    needsFrame(): boolean;
    reset(next: Quaternion, now: number): void;
};
