// The outline of the notch as a vector path (SVG, HOH-41). Pure: the component draws them,
// the tests check them. A corner is a cubic with the control points pulled out (`SMOOTH`), not a circle's
// arc: the curve leaves the straight side gently, as the corner of a phone does.
export interface NotchSize {
  /** Height of the band the notch sits in (--hoh-notch-height = grid line g4), px. */
  readonly band: number;
  readonly emblem: number;
  readonly font: number;
}
/** Below this band height (CSS px) the notch is compact: a smaller emblem and name. */
export const COMPACT_BELOW_BAND = 36;
export function notchSize(band: number): NotchSize {
  return band < COMPACT_BELOW_BAND ? { band, emblem: 16, font: 12 } : { band, emblem: 20, font: 13 };
}

const SMOOTH = 0.66;
const f = (value: number) => Number(value.toFixed(2));
const pt = (x: number, y: number) => `${f(x)} ${f(y)}`;
/** A convex corner from the point `from` to the point `to` around the corner `c`. */
const corner = (from: [number, number], c: [number, number], to: [number, number]) =>
  `C ${pt(from[0] + (c[0] - from[0]) * SMOOTH, from[1] + (c[1] - from[1]) * SMOOTH)} ${pt(to[0] + (c[0] - to[0]) * SMOOTH, to[1] + (c[1] - to[1]) * SMOOTH)} ${pt(to[0], to[1])}`;

export interface Outline {
  readonly width: number;
  readonly height: number;
  /** The closed fill. */
  readonly fill: string;
  /** The hairline: the same outline without the side that lies on the window's edge. */
  readonly line: string;
  /** Where the emblem and the name sit (the box inside the outline). */
  readonly inner: { readonly left: number; readonly top: number; readonly width: number; readonly height: number };
}

/**
 * A notch grown out of the window's top edge: the sides flare into the edge with inverse corners (radius `flare`),
 * the bottom is rounded. `content` is the width of what stands inside (emblem + name), `size` the band. `width` is the
 * width set by grid lines (g9–g13): the padding takes up what the content leaves; without it (narrow window) the notch is as wide as its content.
 */
export function notchOutline(content: number, size: NotchSize, width?: number): Outline {
  const band = size.band;
  const h = band - 1; // the hairline of the notch does not touch the first row of the window
  const flare = Math.round(band * 0.45);
  const radius = Math.round(h * 0.45);
  const natural = Math.round(band * 0.5);
  const pad = width !== undefined && width >= content + flare * 2 + 2 * 8 ? (width - flare * 2 - content) / 2 : natural;
  const body = content + pad * 2;
  const w = body + flare * 2;
  const x0 = flare; const x1 = flare + body;
  // top edge → inverse corner → side → rounded bottom → side → inverse corner → top edge
  const edge = `M 0 0 ${inverseLeft(x0, flare)} V ${f(h - radius)} ${corner([x0, h - radius], [x0, h], [x0 + radius, h])} H ${f(x1 - radius)} ${corner([x1 - radius, h], [x1, h], [x1, h - radius])} V ${f(flare)} ${inverseRight(x1, flare, w)}`;
  return { width: w, height: h, fill: `${edge} Z`, line: edge, inner: { left: x0 + pad, top: flare * 0.4, width: content, height: h - flare * 0.4 } };
}
/** The left inverse corner: leaves the window's edge at (0, 0) horizontally and turns down into the side at (x, y). */
function inverseLeft(x: number, y: number): string {
  return `C ${pt(x * SMOOTH, 0)} ${pt(x, y * (1 - SMOOTH))} ${pt(x, y)}`;
}
/** The right inverse corner: rises from the side at (x, y) and turns along the edge to (w, 0). */
function inverseRight(x: number, y: number, w: number): string {
  return `C ${pt(x, y * (1 - SMOOTH))} ${pt(w - (w - x) * SMOOTH, 0)} ${pt(w, 0)}`;
}
