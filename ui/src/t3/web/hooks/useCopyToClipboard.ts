// Прослойка окна HoH вместо apps/web/src/hooks/useCopyToClipboard.ts (T3 Code, MIT): копирование через navigator.clipboard без сообщений T3.
import { useCallback, useEffect, useRef, useState } from 'react';
export function writeTextToClipboard(text: string, _options?: unknown): Promise<void> { return navigator.clipboard.writeText(text); }
export function useCopyToClipboard<TContext = void>(options: { timeout?: number; onCopy?: (context: TContext) => void; onError?: (error: Error, context: TContext) => void } = {}) {
  const [isCopied, setIsCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const copyToClipboard = useCallback((value: string, context: TContext) => {
    navigator.clipboard.writeText(value).then(() => {
      setIsCopied(true); options.onCopy?.(context);
      clearTimeout(timer.current); timer.current = setTimeout(() => setIsCopied(false), options.timeout ?? 2000);
    }, (error: unknown) => options.onError?.(error instanceof Error ? error : new Error(String(error)), context));
  }, [options]);
  return { copyToClipboard, isCopied };
}
