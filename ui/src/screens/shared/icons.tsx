import type { ReactNode } from 'react';

// Значки варианта A (линейные, 24×24). Каждый — путь без внешних пакетов.
const paths = {
  alert: <><path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" /><path d="M12 8v4M12 16h.01" /></>,
  shield: <path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" />,
  back: <path d="m12 19-7-7 7-7M19 12H5" />,
  check: <path d="M20 6 9 17l-5-5" />,
  checkCircle: <><circle cx="12" cy="12" r="10" /><path d="m9 12 2 2 4-4" /></>,
  chevron: <path d="m9 18 6-6-6-6" />,
  chart: <><path d="M3 3v18h18" /><path d="m7 15 4-4 3 3 5-6" /></>,
  clock: <><circle cx="12" cy="12" r="10" /><path d="M12 6v6l4 2" /></>,
  doc: <><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" /><path d="M14 2v6h6" /></>,
  folder: <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />,
  spinner: <path d="M21 12a9 9 0 1 1-6.2-8.56" />,
  tasks: <><path d="m9 11 3 3L22 4" /><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" /></>,
  user: <><path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" /><circle cx="9" cy="7" r="4" /></>,
} satisfies Record<string, ReactNode>;

export type IconName = keyof typeof paths;

export function Icon({ name, small, spin, className = '' }: { name: IconName; small?: boolean; spin?: boolean; className?: string }) {
  const classes = ['ic', small ? 'ic-sm' : '', spin ? 'ic-spin' : '', className].filter(Boolean).join(' ');
  return <svg className={classes} viewBox="0 0 24 24" aria-hidden="true">{paths[name]}</svg>;
}
