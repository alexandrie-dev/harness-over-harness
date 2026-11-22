// Объявления web/components/device/DeviceWorkspace.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { DeviceHostSummary, DeviceSummary, EnvironmentId } from "@t3tools/contracts";
export declare function DeviceWorkspace(props: {
    environmentId: EnvironmentId;
    device: DeviceSummary;
    hostLabel: string;
    hostKind: DeviceHostSummary["kind"] | undefined;
    hostDiagnostics: string | undefined;
    visible: boolean;
    onFloat: () => void;
    onClose: () => void;
    onPowerOff: () => void;
}): import("react").JSX.Element;
