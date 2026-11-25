// Наложение линий g1..g21 для снимков (HOH-41). Грузится только при сборке с VITE_HOH_GRID_OVERLAY=1 (main.ts); в поставку не входит.
import { lineShare } from './blocks';
import { gridCheck } from './check';

const NS = 'http://www.w3.org/2000/svg';
export function installGridOverlay(): void {
  const layer = document.createElement('div');
  layer.id = 'hoh-grid-overlay';
  layer.setAttribute('aria-hidden', 'true');
  layer.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483000';
  const svg = document.createElementNS(NS, 'svg');
  svg.setAttribute('width', '100%'); svg.setAttribute('height', '100%');
  const badge = document.createElement('div');
  badge.style.cssText = 'position:absolute;right:8px;bottom:8px;padding:2px 6px;font:11px monospace;background:#000;color:#fff;border:1px solid red';
  layer.append(svg, badge);
  document.body.append(layer);
  const line = (x1: number, y1: number, x2: number, y2: number) => {
    const element = document.createElementNS(NS, 'line');
    for (const [key, value] of Object.entries({ x1, y1, x2, y2, stroke: 'red', 'stroke-opacity': '.5', 'stroke-width': '1' })) element.setAttribute(key, String(value));
    svg.append(element);
  };
  const label = (text: string, x: number, y: number) => {
    const element = document.createElementNS(NS, 'text');
    for (const [key, value] of Object.entries({ x, y, fill: 'red', style: 'font:10px monospace' })) element.setAttribute(key, String(value));
    element.textContent = text;
    svg.append(element);
  };
  const draw = () => {
    const width = document.documentElement.clientWidth; const height = window.innerHeight;
    svg.replaceChildren();
    for (let n = 1; n <= 21; n++) {
      const x = lineShare(n) * width; const y = lineShare(n) * height;
      line(x, 0, x, height); label(`g${n}`, Math.min(x + 2, width - 22), 10);
      line(0, y, width, y); label(`g${n}`, 2, Math.min(y - 2, height - 2));
    }
    const misses = gridCheck();
    badge.textContent = misses.length === 0 ? 'grid: ok' : `grid: ${misses.length} off — ${misses.slice(0, 3).map((m) => `${m.block}.${m.side} ${m.px}px`).join('; ')}`;
  };
  draw();
  window.addEventListener('resize', draw);
  setInterval(draw, 1000);
}
