// The outlines of the three notches as vector paths (SVG, HOH-41 design round). Pure: the component draws them,
// the tests check them. A corner is a cubic with the control points pulled out (`SMOOTH`), not a circle's
// arc: the curve leaves the straight side gently, as the corner of a phone does.
export type NotchVariant = 'a' | 'b' | 'c';
export interface NotchSize {
  /** Height of the band the notch sits in (--hoh-notch-height), px. */
  readonly band: number;
  readonly emblem: number;
  readonly font: number;
}
export const NOTCH_SIZES = {
  regular: { band: 36, emblem: 20, font: 13 },
  compact: { band: 30, emblem: 16, font: 12 },
} as const satisfies Record<string, NotchSize>;
/** Below this window height (CSS px) the notch is compact: its band must not eat the conversation. */
export const COMPACT_BELOW_HEIGHT = 640;

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
 * a — a notch grown out of the window's top edge: the sides flare into the edge with inverse corners
 *     (radius `flare`), the bottom is rounded; b — a pill under the edge (a gap, full round ends);
 * c — a bookmark hanging from the edge: straight sides, a deep rounded bottom, no flare.
 * `content` is the width of what stands inside (emblem + name), `size` the band.
 */
export function notchOutline(variant: NotchVariant, content: number, size: NotchSize): Outline {
  const band = size.band;
  if (variant === 'b') {
    const gap = Math.round(band * 0.12);
    const h = band - gap * 2;
    const w = content + h; // a round end each side: padding of h/2
    const r = h / 2; const y0 = gap; const y1 = gap + h;
    const d = `M ${pt(r, y0)} H ${f(w - r)} ${corner([w - r, y0], [w, y0], [w, y0 + r])} ${corner([w, y0 + r], [w, y1], [w - r, y1])} H ${f(r)} ${corner([r, y1], [0, y1], [0, y0 + r])} ${corner([0, y0 + r], [0, y0], [r, y0])} Z`;
    return { width: w, height: band, fill: d, line: d, inner: { left: r, top: y0, width: content, height: h } };
  }
  const h = band - 1; // the hairline of the notch does not touch the first row of the window
  if (variant === 'a') {
    const flare = Math.round(band * 0.5);
    const radius = Math.round(h * 0.5);
    const pad = Math.round(band * 0.5);
    const body = content + pad * 2;
    const w = body + flare * 2;
    const x0 = flare; const x1 = flare + body;
    // top edge → inverse corner → side → rounded bottom → side → inverse corner → top edge
    const edge = `M 0 0 ${inverseLeft(x0, flare)} V ${f(h - radius)} ${corner([x0, h - radius], [x0, h], [x0 + radius, h])} H ${f(x1 - radius)} ${corner([x1 - radius, h], [x1, h], [x1, h - radius])} V ${f(flare)} ${inverseRight(x1, flare, w)}`;
    return { width: w, height: h, fill: `${edge} Z`, line: edge, inner: { left: x0 + pad, top: flare * 0.4, width: content, height: h - flare * 0.4 } };
  }
  const radius = Math.round(h * 0.82);
  const pad = Math.round(band * 0.36);
  const w = content + pad * 2;
  const edge = `M 0 0 V ${f(h - radius)} ${corner([0, h - radius], [0, h], [radius, h])} H ${f(w - radius)} ${corner([w - radius, h], [w, h], [w, h - radius])} V 0`;
  return { width: w, height: h, fill: `${edge} Z`, line: edge, inner: { left: pad, top: 0, width: content, height: h } };
}
/** The left inverse corner: leaves the window's edge at (0, 0) horizontally and turns down into the side at (x, y). */
function inverseLeft(x: number, y: number): string {
  return `C ${pt(x * SMOOTH, 0)} ${pt(x, y * (1 - SMOOTH))} ${pt(x, y)}`;
}
/** The right inverse corner: rises from the side at (x, y) and turns along the edge to (w, 0). */
function inverseRight(x: number, y: number, w: number): string {
  return `C ${pt(x, y * (1 - SMOOTH))} ${pt(w - (w - x) * SMOOTH, 0)} ${pt(w, 0)}`;
}
