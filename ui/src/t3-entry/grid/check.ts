// Проверка сетки в окне (HOH-41): `window.hohGridCheck()` возвращает блоки, чей край стоит дальше TOL px от своей линии
// Golden Canon Grid (пусто — всё на линиях). Карта блоков — blocks.ts, допуск — как в эталоне (edu/app/web/gc-debug.js).
// Её же вызывает сборочная проверка scripts/check-grid.mjs на собранном окне; наложение линий — overlay.ts (только по флагу сборки).
import { BLOCKS, GRID_FROM_WIDTH, lineShare, type Cols } from './blocks';

export const TOL = 2;
export interface Miss { readonly block: string; readonly side: 'left' | 'right' | 'top' | 'bottom' | 'overflow'; readonly px: number; readonly expected: string; readonly nearest: string }

const round = (value: number) => Math.round(value * 10) / 10;
function nearest(value: number, size: number): string {
  let best = 1;
  for (let n = 2; n <= 21; n++) if (Math.abs(value - lineShare(n) * size) < Math.abs(value - lineShare(best) * size)) best = n;
  return `g${best}`;
}
const visible = (element: Element) => {
  const rect = element.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0 && getComputedStyle(element).visibility !== 'hidden';
};

export function gridCheck(): Miss[] {
  const width = document.documentElement.clientWidth;
  const height = window.innerHeight;
  const collapsed = document.querySelector('[data-slot="sidebar-wrapper"]')?.getAttribute('data-sidebar-state') === 'collapsed';
  const misses: Miss[] = [];
  for (const block of BLOCKS) {
    const columns: Cols | null = collapsed ? block.x.collapsed : block.x.open;
    if (!columns) continue;
    for (const element of document.querySelectorAll(block.sel)) {
      if (!visible(element)) continue;
      const rect = element.getBoundingClientRect();
      if (width < GRID_FROM_WIDTH) { // narrow window: no grid, only nothing sticks out past the right edge
        if (rect.right > width + TOL) misses.push({ block: block.name, side: 'overflow', px: round(rect.right - width), expected: 'window', nearest: '' });
        continue;
      }
      const edges: Array<['left' | 'right' | 'top' | 'bottom', number, number, number | null]> = [
        ['left', rect.left, width, columns[0]], ['right', rect.right, width, columns[1]],
        ['top', rect.top, height, block.y?.[0] ?? null], ['bottom', rect.bottom, height, block.y?.[1] ?? null],
      ];
      for (const [side, at, size, line] of edges) {
        if (line === null) continue;
        const off = at - lineShare(line) * size;
        if (Math.abs(off) > TOL) misses.push({ block: block.name, side, px: round(off), expected: `g${line}`, nearest: nearest(at, size) });
      }
    }
  }
  return misses;
}

declare global { interface Window { hohGridCheck?: typeof gridCheck } }
export function installGridCheck(): void { window.hohGridCheck = gridCheck; }
