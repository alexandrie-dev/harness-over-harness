// Объявления web/cloud/managedAuth.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type ReactNode } from "react";
export declare function deactivateManagedRelayAuthentication(): void;
export declare function activateManagedRelayAuthentication(accountId: string, readClerkToken: () => Promise<string | null>): void;
export declare function ManagedRelayAuthProvider({ children }: {
    readonly children: ReactNode;
}): ReactNode;
