// Puts the notch on the window and switches its variant with Ctrl+Alt+1/2/3 (temporary, features.ts: HOH_DESIGN_ROUND).
import { createRoot } from 'react-dom/client';
import './notch.css';
import { HohNotch } from './HohNotch';
import { readHomeVariant, variantOfKey, writeHomeVariant } from './variant';

export function mountNotch(): void {
  document.documentElement.dataset.hohNotch = readHomeVariant();
  const host = document.createElement('div');
  host.id = 'hoh-notch';
  document.body.prepend(host);
  createRoot(host).render(<HohNotch />);
  window.addEventListener('keydown', (event) => {
    const variant = variantOfKey(event);
    if (!variant) return;
    event.preventDefault();
    writeHomeVariant(variant);
  }, true);
}
