// Прослойка окна HoH вместо apps/web/src/components/media/MediaActions.tsx (T3 Code, MIT):
// у окна нет скачивания через хост, поэтому обёртка отдаёт содержимое как есть.
import type { ReactNode } from 'react';
import type { AssetResource, EnvironmentId } from '@t3tools/contracts';
import type { MediaReference } from '@t3tools/client-runtime/media-reference';

export interface MediaActionSource {
  readonly kind: 'image' | 'video';
  readonly name: string;
  readonly src: string | null;
  readonly reference?: MediaReference;
  readonly asset?: { readonly environmentId: EnvironmentId; readonly resource: AssetResource };
  readonly onOpenFile?: () => void;
}
export function MediaActions(props: { readonly source?: MediaActionSource | undefined; readonly children?: ReactNode }) { return props.children; }
