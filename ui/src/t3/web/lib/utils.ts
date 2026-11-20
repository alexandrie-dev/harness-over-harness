// Прослойка окна HoH вместо apps/web/src/lib/utils.ts (T3 Code, MIT): оставлены только функции, которые нужны перенесённому коду.
// Идентификаторы T3 (проекты, потоки, черновики), локализация и Effect здесь не нужны.
import { type CxOptions, cx } from 'class-variance-authority';
import { extendTailwindMerge } from 'tailwind-merge';

// Те же дополнительные размеры текста, что в T3: tailwind-merge не должен принимать text-2xs за цвет.
const twMerge = extendTailwindMerge({
  extend: {
    theme: { text: ['2xs', '3xs', '4xs', '5xs'] },
    classGroups: { 'bg-image': [{ bg: ['composer-seam-below', 'composer-seam-above'] }] },
  },
});

export function cn(...inputs: CxOptions) {
  return twMerge(cx(inputs));
}

export function isMacPlatform(platform: string): boolean {
  return /mac|iphone|ipad|ipod/i.test(platform);
}

export function isWindowsPlatform(platform: string): boolean {
  return /^win(dows)?/i.test(platform);
}
