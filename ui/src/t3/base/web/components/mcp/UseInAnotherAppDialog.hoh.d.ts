// Объявления web/components/mcp/UseInAnotherAppDialog.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type EnvironmentId } from "@t3tools/contracts";
/** Opens the wizard for `environmentId`, or for the one Sharing & access picks when null. */
export declare function openUseInAnotherApp(environmentId: EnvironmentId | null): void;
/** Hosted once at the root, so Cmd-K and Settings can open it from anywhere. */
export declare function UseInAnotherAppDialogHost(): import("react").JSX.Element | null;
