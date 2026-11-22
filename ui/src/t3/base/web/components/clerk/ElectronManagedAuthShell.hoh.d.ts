// Объявления web/components/clerk/ElectronManagedAuthShell.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { ReactNode } from "react";
/** Load only lazily inside desktop; clerk-js is statically bundled and would otherwise enter every
 * client's startup graph. */
export default function ElectronManagedAuthShell({ publishableKey, children, }: {
    readonly publishableKey: string;
    readonly children: ReactNode;
}): import("react").JSX.Element;
