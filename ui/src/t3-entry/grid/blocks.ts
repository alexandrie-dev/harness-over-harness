// Карта блоков окна по линиям Golden Canon Grid (HOH-41): какой блок на каких линиях стоит. Её читают проверка в браузере
// (check.ts, window.hohGridCheck и наложение) и сборочная проверка (scripts/check-grid.mjs). Линии объявлены в tokens.css.
// x — левая и правая линия (g<N> по ширине окна), y — верхняя и нижняя по высоте (null — сторону не проверять).
// Сетка включается с ширины GRID_FROM_WIDTH: уже окна боковая панель T3 (не уже 13rem) и колонка разговора не помещаются
// на линиях; там блоки стоят как у T3, а проверяется только отсутствие выхода за край.
export const GRID_FROM_WIDTH = 1100;
// 20 tracks in 64ths, the same for columns and rows (tokens.css: --gc-tracks): 1, 1, 2, 4, 8/3, 16/3, 16/3, 13/3, 17/6, 7/2 and back.
const HALF = [1, 1, 2, 4, 8 / 3, 16 / 3, 16 / 3, 13 / 3, 17 / 6, 7 / 2];
export const TRACKS: readonly number[] = [...HALF, ...HALF.slice().reverse()];
/** Share of the window (0..1) at line g<n>, n = 1..21. */
export function lineShare(n: number): number {
  return TRACKS.slice(0, n - 1).reduce((sum, track) => sum + track, 0) / 64;
}
export type Cols = readonly [number, number];
export type Rows = readonly [number | null, number | null];
export interface Block {
  readonly name: string;
  /** CSS-селектор блока. */
  readonly sel: string;
  /** Линии по ширине: боковая панель открыта / свёрнута (null — свёрнутой блока нет). */
  readonly x: { readonly open: Cols; readonly collapsed: Cols | null };
  readonly y?: Rows;
}
const same = (x: Cols) => ({ open: x, collapsed: x });
/** Колонка разговора: ширина 50vw по центру области справа от боковой панели (g8–g16 при открытой панели
 *  g1–g6, g7–g15 при свёрнутой). */
const COLUMN = { open: [8, 16], collapsed: [7, 15] } as const;
/** Главная начинается на линии разговора, но использует свободное место справа для новых карточек в том же ряду. */
const HOME = { open: [8, 20], collapsed: [7, 20] } as const;
export const BLOCKS: readonly Block[] = [
  { name: 'notch', sel: '.hoh-notch', x: same([9, 13]), y: [1, 4] },
  { name: 'sidebar', sel: '[data-slot="sidebar-container"]', x: { open: [1, 6], collapsed: null }, y: [4, 21] },
  { name: 'composer', sel: '[data-chat-composer-stack]', x: COLUMN, y: [null, 20] },
  { name: 'home-column', sel: '.hoh-home__inner', x: HOME },
  { name: 'page-column', sel: '.hoh-page__column', x: COLUMN },
  { name: 'home-ask', sel: '.hoh-ask', x: COLUMN },
];
