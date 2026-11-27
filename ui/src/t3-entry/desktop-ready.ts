import { invoke, isTauri } from '@tauri-apps/api/core';
import { useEffect } from 'react';

export function useDesktopUiReady(ready: boolean, legacy = false): void {
  useEffect(() => {
    if (!ready || !isTauri()) return;
    let painted = 0;
    const frame = window.requestAnimationFrame(() => {
      painted = window.requestAnimationFrame(() => {
        void invoke('desktop_ui_ready', { legacy }).catch(() => undefined);
      });
    });
    return () => {
      window.cancelAnimationFrame(frame);
      window.cancelAnimationFrame(painted);
    };
  }, [ready, legacy]);
}
