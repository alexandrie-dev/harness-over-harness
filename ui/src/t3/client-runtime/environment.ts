// Прослойка окна HoH вместо packages/client-runtime/src/environment/index.ts (T3 Code, MIT): 
export const scopedThreadKey = (ref: { environmentId: string; threadId: string }): string => `${ref.environmentId}:${ref.threadId}`;
