// Объявления web/components/clerk/BrowserManagedAuthShell.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import type { ReactNode } from "react";
/** Lazy-load Clerk so cloudless local mode never downloads it. */
export default function BrowserManagedAuthShell({ publishableKey, children, }: {
    readonly publishableKey: string;
    readonly children: ReactNode;
}): import("react").JSX.Element;
