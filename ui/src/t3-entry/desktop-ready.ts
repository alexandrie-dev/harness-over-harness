import { invoke, isTauri } from '@tauri-apps/api/core';
import { useEffect } from 'react';

let nextView = 0;

export function useDesktopUiReady(ready: boolean, legacy = false): void {
  useEffect(() => {
    if (!ready || !isTauri()) return;
    const view = ++nextView;
    let painted = 0;
    let reported = false;
    const frame = window.requestAnimationFrame(() => {
      painted = window.requestAnimationFrame(() => {
        reported = true;
        void invoke('desktop_ui_ready', { legacy, view, ready: true }).catch(() => undefined);
      });
    });
    return () => {
      window.cancelAnimationFrame(frame);
      window.cancelAnimationFrame(painted);
      if (reported) {
        void invoke('desktop_ui_ready', { legacy, view, ready: false }).catch(() => undefined);
      }
    };
  }, [ready, legacy]);
}
