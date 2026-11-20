// Прослойка окна HoH вместо apps/web/src/components/ui/toast.tsx (T3 Code, MIT): всплывающих сообщений T3 нет; ошибки показывает вызывающий код в разметке.
export const toastManager = { add: (_toast: unknown): string => '', close: (_id?: string): void => {} };
export const stackedThreadToast = (toast: unknown): unknown => toast;
