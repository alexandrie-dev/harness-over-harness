// The variant of the home screen and of the notch (design round, HOH-41): a, b or c. Temporary, not shipped:
// the choice is kept in the window's localStorage and changed by Ctrl+Alt+1/2/3 or by Settings → Appearance
// («Home variant»). One flag turns the whole round off (features.ts, HOH_DESIGN_ROUND).
export type HomeVariant = 'a' | 'b' | 'c';
export const HOME_VARIANTS: readonly HomeVariant[] = ['a', 'b', 'c'];
export const HOME_VARIANT_KEY = 'hoh.homeVariant';
export const HOME_VARIANT_EVENT = 'hoh-home-variant';

export function readHomeVariant(storage: Pick<Storage, 'getItem'> | null = safeStorage()): HomeVariant {
  const value = storage?.getItem(HOME_VARIANT_KEY);
  return value === 'b' || value === 'c' ? value : 'a';
}
export function writeHomeVariant(variant: HomeVariant, storage: Pick<Storage, 'setItem'> | null = safeStorage(), target: EventTarget = window): void {
  try { storage?.setItem(HOME_VARIANT_KEY, variant); } catch { /* storage unavailable: the choice lasts until the window closes */ }
  target.dispatchEvent(new CustomEvent(HOME_VARIANT_EVENT, { detail: variant }));
}
/** Calls `listener` with the new variant when it changes (any part of the window); returns the way to stop. */
export function subscribeHomeVariant(listener: (variant: HomeVariant) => void, target: EventTarget = window): () => void {
  const handler = (event: Event) => listener((event as CustomEvent<HomeVariant>).detail);
  target.addEventListener(HOME_VARIANT_EVENT, handler);
  return () => target.removeEventListener(HOME_VARIANT_EVENT, handler);
}
/** Ctrl+Alt+1/2/3. */
export function variantOfKey(event: Pick<KeyboardEvent, 'ctrlKey' | 'altKey' | 'metaKey' | 'shiftKey' | 'code'>): HomeVariant | null {
  if (!event.ctrlKey || !event.altKey || event.metaKey || event.shiftKey) return null;
  return ({ Digit1: 'a', Digit2: 'b', Digit3: 'c' } as Record<string, HomeVariant>)[event.code] ?? null;
}
function safeStorage(): Storage | null {
  try { return window.localStorage; } catch { return null; }
}
