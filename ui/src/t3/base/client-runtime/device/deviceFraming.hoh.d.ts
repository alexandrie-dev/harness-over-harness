// Объявления client-runtime/device/deviceFraming.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { Box3 } from "three";
export declare function createDeviceFraming(): {
    center: any;
    setBounds(next: Box3, vertical: number, aspect: number, now: number, immediate?: boolean): void;
    advance: (now: number, immediate?: boolean) => boolean;
    distance(): number;
    hold(active: boolean, now: number): void;
    needsFrame(): boolean;
};
