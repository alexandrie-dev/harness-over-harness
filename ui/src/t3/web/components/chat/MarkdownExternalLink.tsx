// Прослойка окна HoH вместо apps/web/src/components/chat/MarkdownExternalLink.tsx (T3 Code, MIT): значок сайта берёт T3 у системы контроля версий; здесь — просто текст ссылки.
import type { ReactNode } from 'react';
export function MarkdownExternalLinkContent(props: { readonly children?: ReactNode; readonly [key: string]: unknown }) { return props.children; }
