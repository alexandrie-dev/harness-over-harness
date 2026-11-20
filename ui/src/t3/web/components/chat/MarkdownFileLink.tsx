// Прослойка окна HoH вместо apps/web/src/components/chat/MarkdownFileLink.tsx (T3 Code, MIT): чип файла T3 опирается на хост (редактор, проводник, значки Pierre); здесь файл показывается кодом.
import type { ReactNode } from 'react';
export function MarkdownFileLink(props: { readonly children?: ReactNode; readonly [key: string]: unknown }) { return <code>{props.children}</code>; }
