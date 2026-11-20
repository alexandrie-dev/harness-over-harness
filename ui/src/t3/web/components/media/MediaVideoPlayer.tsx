// Прослойка окна HoH вместо apps/web/src/components/media/MediaVideoPlayer.tsx (T3 Code, MIT): обычный <video>.
import type { CSSProperties } from 'react';
import type { MediaActionSource } from './MediaActions';

export function MediaVideoPlayer(props: {
  readonly src: string | null; readonly label?: string; readonly sourceFailed?: boolean | undefined;
  readonly unavailableDetail?: string | undefined; readonly originalUrl?: string | undefined; readonly style?: CSSProperties | undefined;
  readonly copyMarkdown?: string | undefined; readonly className?: string | undefined; readonly videoClassName?: string | undefined;
  readonly onRetry?: (() => Promise<unknown>) | undefined; readonly actionsSource?: MediaActionSource | undefined;
}) {
  return props.src && !props.sourceFailed
    ? <video className={props.videoClassName} style={props.style} src={props.src} controls preload="metadata" aria-label={props.label} />
    : <p className={props.className}>{props.unavailableDetail ?? props.label}</p>;
}
