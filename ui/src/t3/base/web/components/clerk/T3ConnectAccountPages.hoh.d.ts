// Объявления web/components/clerk/T3ConnectAccountPages.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { type ReactNode } from "react";
export declare const T3_CONNECT_ACCOUNT_PAGES: readonly [{
    readonly label: "Mobile clients";
    readonly url: "mobile-clients";
    readonly icon: import("react").JSX.Element;
    readonly content: import("react").JSX.Element;
}, {
    readonly label: "T3 Connect";
    readonly url: "t3-connect";
    readonly icon: import("react").JSX.Element;
    readonly content: import("react").JSX.Element;
}, {
    readonly label: "T3 Chat";
    readonly url: "t3-chat";
    readonly icon: import("react").JSX.Element;
    readonly content: import("react").JSX.Element;
}];
export type T3ConnectAccountPageUrl = (typeof T3_CONNECT_ACCOUNT_PAGES)[number]["url"];
/** Keep portals rendered while the modal can open; Clerk owns the custom-page DOM nodes. */
export declare function useT3ConnectAccountPage(): {
    readonly open: ((page?: T3ConnectAccountPageUrl) => void) | null;
    readonly portals: ReactNode;
};
