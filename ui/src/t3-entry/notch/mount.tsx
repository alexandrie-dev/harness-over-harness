// Puts the notch on the window (HOH-41): the band above the app and the outline a, a flare grown out of the top edge.
import { createRoot } from 'react-dom/client';
import './notch.css';
import { HohNotch } from './HohNotch';

export function mountNotch(): void {
  document.documentElement.dataset.hohNotch = '';
  const host = document.createElement('div');
  host.id = 'hoh-notch';
  document.body.prepend(host);
  createRoot(host).render(<HohNotch />);
}
