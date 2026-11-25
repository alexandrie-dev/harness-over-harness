// Runtime half of the HoH grid check. T3 owns the external axis and width; this guard checks that HoH-owned leaves
// remain inside their nearest home/plugin lane and the viewport. Golden Canon lines remain an internal rhythm guide.
import { BLOCKS } from './blocks';

export const TOL = 2;
export interface Miss { readonly block: string; readonly side: 'left' | 'right' | 'overflow'; readonly px: number; readonly expected: string; readonly nearest: string }

const round = (value: number) => Math.round(value * 10) / 10;
const visible = (element: Element) => {
  const rect = element.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0 && getComputedStyle(element).visibility !== 'hidden';
};

export function gridCheck(): Miss[] {
  const width = document.documentElement.clientWidth;
  const misses: Miss[] = [];
  for (const block of BLOCKS) {
    for (const element of document.querySelectorAll(block.sel)) {
      if (!visible(element)) continue;
      const rect = element.getBoundingClientRect();
      const lane = element.closest('.hoh-home__inner, .hoh-plugins, .hoh-page__column')?.getBoundingClientRect();
      const left = lane?.left ?? 0;
      const right = lane?.right ?? width;
      if (rect.left < left - TOL) misses.push({ block: block.name, side: 'left', px: round(rect.left - left), expected: 'T3/HoH lane', nearest: '' });
      if (rect.right > right + TOL) misses.push({ block: block.name, side: 'right', px: round(rect.right - right), expected: 'T3/HoH lane', nearest: '' });
      if (rect.left < -TOL || rect.right > width + TOL)
        misses.push({ block: block.name, side: 'overflow', px: round(Math.max(-rect.left, rect.right - width)), expected: 'window', nearest: '' });
    }
  }
  return misses;
}

declare global { interface Window { hohGridCheck?: typeof gridCheck } }
export function installGridCheck(): void { window.hohGridCheck = gridCheck; }
