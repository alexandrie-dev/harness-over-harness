// Объявления web/components/device/DeviceControlsRail.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { DeviceHostSummary, DevicePlatform } from "@t3tools/contracts";
import type { DeviceStreamHandle, DeviceViewControls } from "./DeviceStreamView";
import type { DeviceControls } from "./useDeviceControls";
export declare function DeviceControlsRail(props: {
    platform: DevicePlatform;
    hostKind: DeviceHostSummary["kind"] | undefined;
    handle: DeviceStreamHandle | null;
    view: DeviceViewControls;
    controls: DeviceControls;
    screenshotPending: boolean;
    onScreenshot: () => void;
    toolsOpen: boolean;
    onTools: () => void;
    onFloat: () => void;
    onClose: () => void;
    onPowerOff: () => void;
}): import("react").JSX.Element;
