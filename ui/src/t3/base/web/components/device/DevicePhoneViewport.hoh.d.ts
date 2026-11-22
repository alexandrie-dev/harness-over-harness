// Объявления web/components/device/DevicePhoneViewport.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { DeviceAccessorySource, DeviceModelSource } from "@t3tools/client-runtime/device/model";
import type { DeviceShapeProfile } from "@t3tools/client-runtime/device/shape-profile";
import type { DeviceScreenSize, DeviceStreamClient } from "@t3tools/client-runtime/device/stream";
export declare function DevicePhoneViewport(props: {
    readonly profile: DeviceShapeProfile;
    readonly model: DeviceModelSource | null;
    readonly accessory: DeviceAccessorySource | null;
    readonly source: HTMLCanvasElement;
    readonly onFrameListener: (listener: (() => void) | null) => void;
    readonly onResetReady: (reset: (() => void) | null) => void;
    readonly onInputCancel: (cancel: (() => void) | null) => void;
    readonly client: DeviceStreamClient;
    readonly screen: DeviceScreenSize | null;
    readonly foldAngle: number | null;
    readonly onUnavailable: () => void;
}): import("react").JSX.Element;
