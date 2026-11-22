// Объявления web/components/device/DeviceStreamView.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { DevicePlatform, EnvironmentId } from "@t3tools/contracts";
import { type ReactNode } from "react";
import type { DeviceHardwareButton } from "@t3tools/client-runtime/device/stream";
export interface DeviceViewControls {
    readonly phone: boolean;
    readonly streaming: boolean;
    readonly phoneUnavailableReason: string | null;
    readonly showPhone: () => void;
    readonly showFlat: () => void;
    readonly resetView: () => void;
    readonly foldingControls?: ReactNode;
    readonly keyboard: {
        readonly attached: boolean;
        readonly toggle: () => void;
    } | null;
    readonly handle: DeviceStreamHandle | null;
}
export interface DeviceStreamHandle {
    readonly pressButton: (button: DeviceHardwareButton) => void;
    readonly rotate: () => void;
    readonly inputConnected: boolean;
}
export declare function DeviceStreamView(props: {
    readonly environmentId: EnvironmentId;
    readonly platform: DevicePlatform;
    readonly deviceId: string;
    readonly deviceName?: string;
    readonly deviceDescription?: string;
    readonly visible: boolean;
    readonly hostId: string;
    readonly allowPhoneView?: boolean;
    readonly renderControls?: (view: DeviceViewControls) => ReactNode;
    readonly axOverlay?: boolean;
    /** Lets a parent read this view's own stream state, such as its screen size. */
    readonly viewId?: string;
    /** The host serves only serve-sim's stream and input socket (Simlock). */
    readonly serveSimOnly?: boolean;
}): import("react").JSX.Element;
