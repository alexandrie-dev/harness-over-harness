// Прослойка окна HoH вместо apps/web/src/components/chat/ExpandedImagePreview.ts (T3 Code, MIT):
// только типы просмотра изображений; разворот на весь экран окну не нужен.
import type { MediaActionSource } from '../media/MediaActions';

export interface ExpandedImageItem {
  src: string | null;
  name: string;
  mimeType?: string;
  type?: 'video';
  actionsSource?: MediaActionSource | undefined;
}
export interface ExpandedImagePreview { images: ExpandedImageItem[]; index: number }
