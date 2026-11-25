// Объявления web/components/device/DevicePanel.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { ScopedThreadRef } from "@t3tools/contracts";
import { type RightPanelSurface } from "~/rightPanelStore";
import { type PreviewPanelMode } from "../preview/PreviewPanelShell";
export declare function DevicePanel(props: {
    readonly mode: PreviewPanelMode;
    readonly threadRef: ScopedThreadRef;
    readonly surface: Extract<RightPanelSurface, {
        kind: "device";
    }>;
    readonly visible: boolean;
    readonly onDismissSetup: () => void;
}): import("react").JSX.Element;
