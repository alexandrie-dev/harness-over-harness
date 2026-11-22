// Объявления web/components/CustomSnoozeDialog.tsx (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
type SnoozeChoice = {
    readonly snoozedUntil: string;
};
export declare function requestCustomSnooze(): Promise<SnoozeChoice | null>;
export declare function CustomSnoozeDialogHost(): import("react").JSX.Element | null;
export {};
