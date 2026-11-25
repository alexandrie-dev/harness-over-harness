// Golden Canon Grid remains an internal design aid for HoH-owned blocks. External window geometry is T3's native
// sidebar/content/chat layout and is verified by scripts/check-align.mjs instead of viewport grid lines.
export const GRID_FROM_WIDTH = 1100;
// 20 tracks in 64ths, the same for columns and rows (tokens.css: --gc-tracks): 1, 1, 2, 4, 8/3, 16/3, 16/3, 13/3, 17/6, 7/2 and back.
const HALF = [1, 1, 2, 4, 8 / 3, 16 / 3, 16 / 3, 13 / 3, 17 / 6, 7 / 2];
export const TRACKS: readonly number[] = [...HALF, ...HALF.slice().reverse()];
/** Share of the window (0..1) at line g<n>, n = 1..21. */
export function lineShare(n: number): number {
  return TRACKS.slice(0, n - 1).reduce((sum, track) => sum + track, 0) / 64;
}
export interface Block {
  readonly name: string;
  readonly sel: string;
}
/** HoH-owned leaves whose runtime check guards containment in their nearest T3/HoH lane. */
export const BLOCKS: readonly Block[] = [
  { name: 'home-card', sel: '.hoh-card' },
  { name: 'project-folder', sel: '.hoh-project' },
  { name: 'project-disclosure', sel: '.hoh-project-popover' },
  { name: 'plugin-block', sel: '.hoh-plugin-card, .hoh-plugin-row, .hoh-plugin-detail' },
];
