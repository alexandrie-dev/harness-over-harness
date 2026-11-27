import { useRouterState } from '@tanstack/react-router';
import { useDesktopUiReady } from './desktop-ready';

export function DesktopUpdateReady() {
  const ready = useRouterState({
    select: (state) => state.status === 'idle' && !state.isLoading && state.matches.length > 0
      && state.matches.every((match) => match.status === 'success' && !match.globalNotFound),
  });
  useDesktopUiReady(ready);
  return null;
}
