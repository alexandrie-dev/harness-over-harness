// Объявления web/components/device/DeviceDuoViewport.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { DeviceModelSource } from "@t3tools/client-runtime/device/model";
import type { DeviceScreenSize, DeviceStreamClient } from "@t3tools/client-runtime/device/stream";
export declare function DeviceDuoViewport(props: {
    readonly model: DeviceModelSource;
    readonly source: HTMLCanvasElement;
    readonly onFrameListener: (listener: (() => void) | null) => void;
    readonly onResetReady: (reset: (() => void) | null) => void;
    readonly onInputCancel: (cancel: (() => void) | null) => void;
    readonly client: DeviceStreamClient;
    readonly screen: DeviceScreenSize | null;
    readonly hingePreview: number | null;
    readonly onControlRejectedListener: (listener: (() => void) | null) => void;
    readonly onUnavailable: () => void;
}): import("react").JSX.Element;
