// The notch at the top centre of the window (HOH-41 design round): the emblem and «Alexandrie» stand inside a vector
// outline — a — grown out of the top edge with inverse corners, b — a pill under the edge, c — a bookmark hanging
// from the edge. It sits in a band of its own above the app (--hoh-notch-height, notch.css), so it never covers the
// header of a conversation or the buttons of the sidebar and the right panel, whatever the width of the sidebar is.
// The outline is a hairline in T3's border colour over T3's card surface; no glow, shadow or gradient.
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import e1 from '../../assets/emblem-notch-1x.png';
import e15 from '../../assets/emblem-notch-15x.png';
import e2 from '../../assets/emblem-notch-2x.png';
import e3 from '../../assets/emblem-notch-3x.png';
import s1 from '../../assets/emblem-notchs-1x.png';
import s15 from '../../assets/emblem-notchs-15x.png';
import s2 from '../../assets/emblem-notchs-2x.png';
import s3 from '../../assets/emblem-notchs-3x.png';
import { COMPACT_BELOW_HEIGHT, NOTCH_SIZES, notchOutline } from './shapes';
import { readHomeVariant, subscribeHomeVariant, type HomeVariant } from './variant';

export const NOTCH_NAME = 'Alexandrie';
const GAP = 8;

function useCompact(): boolean {
  const query = `(max-height: ${COMPACT_BELOW_HEIGHT - 1}px)`;
  const [compact, setCompact] = useState(() => window.matchMedia(query).matches);
  useEffect(() => {
    const media = window.matchMedia(query);
    const update = () => setCompact(media.matches);
    media.addEventListener('change', update);
    update();
    return () => media.removeEventListener('change', update);
  }, [query]);
  return compact;
}

/** `variant` fixed by the caller (the screens of the round), else the one the person chose (Ctrl+Alt+1/2/3). */
export function HohNotch({ variant }: { readonly variant?: HomeVariant }) {
  const [chosen, setChosen] = useState<HomeVariant>(readHomeVariant);
  useEffect(() => subscribeHomeVariant(setChosen), []);
  const shape = variant ?? chosen;
  const size = NOTCH_SIZES[useCompact() ? 'compact' : 'regular'];
  const label = useRef<HTMLSpanElement>(null);
  const [text, setText] = useState(size.font * 5.4);
  useLayoutEffect(() => {
    const measure = () => { if (label.current) setText(label.current.getBoundingClientRect().width); };
    measure();
    void document.fonts?.ready.then(measure);
  }, [size.font]);
  const content = size.emblem * (19 / 20) + GAP + Math.ceil(text);
  const outline = notchOutline(shape, content, size);
  const small = size === NOTCH_SIZES.compact;
  const [one, onefive, two, three] = small ? [s1, s15, s2, s3] : [e1, e15, e2, e3];
  return (
    <div className="hoh-notch-band" data-hoh-notch-variant={shape}>
      <div className="hoh-notch" style={{ width: outline.width, height: outline.height }}>
        <svg className="hoh-notch-outline" width={outline.width} height={outline.height} viewBox={`0 0 ${outline.width} ${outline.height}`} aria-hidden="true">
          <path className="hoh-notch-fill" d={outline.fill} />
          <path className="hoh-notch-line" d={outline.line} />
        </svg>
        <div className="hoh-notch-content" style={{ left: outline.inner.left, top: outline.inner.top, width: outline.inner.width, height: outline.inner.height, gap: GAP }}>
          <img
            src={one} srcSet={`${one} 1x, ${onefive} 1.5x, ${two} 2x, ${three} 3x`}
            width={Math.round(size.emblem * 0.95)} height={size.emblem} alt="" draggable={false}
          />
          <span ref={label} className="hoh-notch-name" style={{ fontSize: size.font }}>{NOTCH_NAME}</span>
        </div>
      </div>
    </div>
  );
}
