// TanStack Router eagerly reads sessionStorage while its module is evaluated, even
// when scroll restoration is disabled. WebKitGTK can block the main thread on
// that first read. HoH does not enable router scroll restoration; keep the real
// storage behavior when a router or its restoration hook actually requests it.
export function deferRouterScrollStorage(code: string): string {
  const edits: ReadonlyArray<readonly [string, string]> = [
    ['const safeSessionStorage = getSafeSessionStorage();', 'let safeSessionStorage;'],
    [
      'const scrollRestorationCache = /* @__PURE__ */ createScrollRestorationCache();',
      `let scrollRestorationCache = {};
let scrollRestorationCacheInitialized = false;
function ensureScrollRestorationCache() {
  if (scrollRestorationCacheInitialized) return;
  scrollRestorationCacheInitialized = true;
  safeSessionStorage = getSafeSessionStorage();
  scrollRestorationCache = createScrollRestorationCache();
}`,
    ],
    [
      'function getElementScrollRestorationEntry(router, options) {',
      'function getElementScrollRestorationEntry(router, options) {\n  ensureScrollRestorationCache();',
    ],
    [
      'if (force ?? router.options.scrollRestoration) router.isScrollRestoring = true;',
      'if (force ?? router.options.scrollRestoration) { ensureScrollRestorationCache(); router.isScrollRestoring = true; }',
    ],
  ];
  for (const [before, after] of edits) {
    if (code.indexOf(before) < 0 || code.indexOf(before) !== code.lastIndexOf(before)) {
      throw new Error(`TanStack scroll-restoration.js changed: expected exactly one ${JSON.stringify(before)}`);
    }
    code = code.replace(before, after);
  }
  return code;
}
