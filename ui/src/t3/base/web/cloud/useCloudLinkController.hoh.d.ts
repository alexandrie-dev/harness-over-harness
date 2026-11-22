// Объявления web/cloud/useCloudLinkController.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type EnvironmentId } from "@t3tools/contracts";
export interface CloudLinkDesiredState {
    readonly managedTunnel: boolean;
    readonly publish: boolean;
    readonly holdWebhooksWhileOffline?: boolean;
}
export declare function useCloudLinkController(environmentId: EnvironmentId | null): {
    isSignedIn: any;
    linkState: {
        data: {
            readonly linked: boolean;
            readonly cloudUserId: string | null;
            readonly relayIssuer: string | null;
            readonly relayUrl: string | null;
            readonly publishAgentActivity: boolean;
            readonly holdWebhooksWhileOffline?: boolean | undefined;
            readonly managedTunnelActive?: boolean | undefined;
            readonly linkRevoked?: boolean | undefined;
            readonly reachability?: {
                readonly status: "reachable";
                readonly checkedAt: string;
                readonly readyConnections: number | null;
                readonly protocol: string | null;
            } | {
                readonly status: "unreachable";
                readonly checkedAt: string;
                readonly cause: "timeout" | "origin_unreachable" | "not_linked" | "connector_not_running" | "connector_not_ready" | "edge_unreachable" | "no_connector" | "dns_failed" | "unexpected_response";
                readonly reason: string;
                readonly hint: string | null;
                readonly fix: string;
            } | undefined;
            readonly tunnelHostname?: string | undefined;
            readonly linkableHere?: boolean | undefined;
        } | null;
        error: string | null;
        isPending: boolean;
        refresh: () => void;
        target: import("./linkEnvironment").CloudLinkTarget | null;
    };
    linked: boolean;
    linkRevoked: boolean;
    managedTunnelActive: boolean;
    managedTunnelOutOfSync: boolean;
    publishAgentActivity: boolean;
    holdWebhooksWhileOffline: boolean;
    operationError: string | null;
    tunnelLimit: number | null;
    needsRelayConfiguration: (desired: CloudLinkDesiredState) => boolean;
    reconcileCloudState: (desired: CloudLinkDesiredState) => Promise<boolean>;
};
