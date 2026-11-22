// The notch at the top centre of the window (HOH-41): the emblem and «Alexandrie» stand inside a vector outline
// grown out of the top edge with inverse corners (the founder's choice, shape a; still a draft outline). It sits in a band of its own above the app (--hoh-notch-height, notch.css), so it never covers the
// header of a conversation or the buttons of the sidebar and the right panel, whatever the width of the sidebar is.
// The outline is a hairline in T3's border colour over T3's card surface; no glow, shadow or gradient.
import { useLayoutEffect, useRef, useState } from 'react';
import e1 from '../../assets/emblem-notch-1x.png';
import e15 from '../../assets/emblem-notch-15x.png';
import e2 from '../../assets/emblem-notch-2x.png';
import e3 from '../../assets/emblem-notch-3x.png';
import s1 from '../../assets/emblem-notchs-1x.png';
import s15 from '../../assets/emblem-notchs-15x.png';
import s2 from '../../assets/emblem-notchs-2x.png';
import s3 from '../../assets/emblem-notchs-3x.png';
import { BLOCKS, GRID_FROM_WIDTH, lineShare } from '../grid/blocks';
import { notchOutline, notchSize } from './shapes';

export const NOTCH_NAME = 'Alexandrie';
const GAP = 8;
const NOTCH_LINES = BLOCKS.find((block) => block.name === 'notch')!.x.open;
/** The band's height (CSS px, = grid line g4) and the window's width: the notch is drawn from them. */
function useFrame(band: React.RefObject<HTMLDivElement | null>): { band: number; width: number } {
  const [frame, setFrame] = useState({ band: 36, width: window.innerWidth });
  useLayoutEffect(() => {
    const element = band.current;
    if (!element) return;
    const update = () => setFrame((was) => {
      const next = { band: element.getBoundingClientRect().height || was.band, width: window.innerWidth };
      return next.band === was.band && next.width === was.width ? was : next;
    });
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    window.addEventListener('resize', update);
    return () => { observer.disconnect(); window.removeEventListener('resize', update); };
  }, [band]);
  return frame;
}

export function HohNotch() {
  const bandRef = useRef<HTMLDivElement>(null);
  const frame = useFrame(bandRef);
  const size = notchSize(frame.band);
  const lineWidth = frame.width >= GRID_FROM_WIDTH ? (lineShare(NOTCH_LINES[1]!) - lineShare(NOTCH_LINES[0])) * frame.width : undefined;
  const label = useRef<HTMLSpanElement>(null);
  const [text, setText] = useState(size.font * 5.4);
  useLayoutEffect(() => {
    const measure = () => { if (label.current) setText(label.current.getBoundingClientRect().width); };
    measure();
    void document.fonts?.ready.then(measure);
  }, [size.font]);
  const content = size.emblem * (28 / 30) + GAP + Math.ceil(text);
  const outline = notchOutline(content, size, lineWidth);
  const small = size.emblem < 30;
  const [one, onefive, two, three] = small ? [s1, s15, s2, s3] : [e1, e15, e2, e3];
  return (
    <div className="hoh-notch-band" ref={bandRef}>
      <div className="hoh-notch" style={{ width: outline.width, height: outline.height }}>
        <svg className="hoh-notch-outline" width={outline.width} height={outline.height} viewBox={`0 0 ${outline.width} ${outline.height}`} aria-hidden="true">
          <path className="hoh-notch-fill" d={outline.fill} />
          <path className="hoh-notch-line" d={outline.line} />
        </svg>
        <div className="hoh-notch-content" style={{ left: outline.inner.left, top: outline.inner.top, width: outline.inner.width, height: outline.inner.height, gap: GAP }}>
          <img
            src={one} srcSet={`${one} 1x, ${onefive} 1.5x, ${two} 2x, ${three} 3x`}
            width={Math.round(size.emblem * (28 / 30))} height={size.emblem} alt="" draggable={false}
          />
          <span ref={label} className="hoh-notch-name" style={{ fontSize: size.font }}>{NOTCH_NAME}</span>
        </div>
      </div>
    </div>
  );
}
