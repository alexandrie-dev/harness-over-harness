import { memo, useId } from 'react';

const TILE_SIZE = 10;
const GROOVE_EDGE = 'M0 1.5h10';
const GROOVE_ROTATION = 'rotate(45)';

export const SteelBackdrop = memo(function SteelBackdrop() {
  const id = useId();
  const texture = `${id}-texture`;
  const shade = `${id}-shade`;
  const pulse = `${id}-pulse`;
  const edges = `${id}-edges`;
  const edgeMask = `${id}-edge-mask`;

  return (
    <svg data-hoh-steel-backdrop="" aria-hidden="true" focusable="false">
      <defs>
        <pattern
          id={texture}
          width={TILE_SIZE}
          height={TILE_SIZE}
          patternUnits="userSpaceOnUse"
          patternTransform={GROOVE_ROTATION}
          data-steel-texture=""
        >
          <path d="M0 0.5h10" stroke="#080808" />
          <path d={GROOVE_EDGE} stroke="#282828" />
        </pattern>
        <radialGradient id={shade} cx="57%" cy="43%" r="68%">
          <stop stopColor="#000" />
          <stop offset="42%" stopColor="#000" stopOpacity={0.97} />
          <stop offset="74%" stopColor="#000" stopOpacity={0.58} />
          <stop offset="100%" stopColor="#000" stopOpacity={0.23} />
        </radialGradient>
        <pattern
          id={edges}
          width={TILE_SIZE}
          height={TILE_SIZE}
          patternUnits="userSpaceOnUse"
          patternTransform={GROOVE_ROTATION}
        >
          <path d={GROOVE_EDGE} stroke="white" />
        </pattern>
        <mask id={edgeMask} x={0} y={0} width="100%" height="100%" maskUnits="userSpaceOnUse">
          <rect width="100%" height="100%" fill={`url(#${edges})`} />
        </mask>
        <radialGradient id={pulse}>
          <stop stopColor="currentColor" />
          <stop offset="100%" stopColor="currentColor" stopOpacity={0} />
        </radialGradient>
      </defs>
      <rect width="100%" height="100%" fill="#000" />
      <rect width="100%" height="100%" fill={`url(#${texture})`} />
      <rect width="100%" height="100%" fill={`url(#${shade})`} />
      <g mask={`url(#${edgeMask})`}>
        <ellipse data-steel-pulse="" cx="85%" cy="77%" rx={62} ry={22} fill={`url(#${pulse})`} />
      </g>
    </svg>
  );
});
