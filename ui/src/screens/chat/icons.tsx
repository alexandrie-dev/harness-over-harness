import type { ReactNode } from 'react';
const paths = {
  lock: <><rect x="4" y="10" width="16" height="11" rx="2" /><path d="M8 10V6a4 4 0 0 1 8 0v4" /></>,
  up: <path d="M12 19V5m-6 6 6-6 6 6" />,
  stop: <rect className="chat-stop-square" x="7" y="7" width="10" height="10" rx="1" />,
  star: <path d="m12 3 2.6 6.4L21 12l-6.4 2.6L12 21l-2.6-6.4L3 12l6.4-2.6L12 3Z" />,
} satisfies Record<string, ReactNode>;
export function ChatIcon({ name, small = false }: { name: keyof typeof paths; small?: boolean }) {
  return <svg className={small ? 'ic ic-sm' : 'ic'} viewBox="0 0 24 24" aria-hidden="true">{paths[name]}</svg>;
}
