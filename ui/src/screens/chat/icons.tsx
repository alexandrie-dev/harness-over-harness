import type { ReactNode } from 'react';
const paths = {
  lock: <><rect x="4" y="10" width="16" height="11" rx="2" /><path d="M8 10V6a4 4 0 0 1 8 0v4" /></>,
  up: <path d="m5 12 7-7 7 7M12 19V5" />,
  stop: <rect className="chat-stop-square" x="7" y="7" width="10" height="10" rx="1" />,
  star: <path d="m12 3 2.6 6.4L21 12l-6.4 2.6L12 21l-2.6-6.4L3 12l6.4-2.6L12 3Z" />,
  newChat: <><path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" /><path d="M18.375 2.625a1 1 0 0 1 3 3l-9.013 9.014a2 2 0 0 1-.853.505l-2.873.84a.5.5 0 0 1-.62-.62l.84-2.873a2 2 0 0 1 .506-.852z" /></>,
} satisfies Record<string, ReactNode>;
export function ChatIcon({ name, small = false }: { name: keyof typeof paths; small?: boolean }) {
  return <svg className={small ? 'ic ic-sm' : 'ic'} viewBox="0 0 24 24" aria-hidden="true">{paths[name]}</svg>;
}
