// Эмблема над заголовком пустого разговора (brand.css) показывается, только если над заголовком хватает места между
// шапкой окна и самим заголовком (HOH-41). Эмблема лежит вне потока (position: absolute над блоком заголовка), поэтому
// не двигает ни заголовок, ни поле ввода и не уходит под шапку: места нет — атрибут data-hoh-emblem не ставится, и она скрыта.
// Правило считается по настоящим размерам (в CSS px, то есть с учётом масштаба окна Ctrl+«+»), а не по порогу высоты окна.
const HEADINGS = 'div.mx-auto.max-w-5xl.flex-col.items-center:has(> h1, > div[data-inline]), [data-slot="empty-header"]:has(> [data-slot="empty-title"])';
// Высота эмблемы 104 + отступ до заголовка 22 + зазор до шапки 8 (brand.css).
const NEED = 104 + 22 + 8;

function headerBottom(): number {
  const topbar = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--workspace-topbar-height'));
  return Number.isFinite(topbar) ? topbar : 52;
}

function fit(): void {
  const top = headerBottom();
  for (const heading of document.querySelectorAll<HTMLElement>(HEADINGS)) {
    heading.toggleAttribute('data-hoh-emblem', heading.getBoundingClientRect().top - top >= NEED);
  }
}

export function startEmblemFit(): void {
  let queued = false;
  const schedule = () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => { queued = false; fit(); });
  };
  window.addEventListener('resize', schedule);
  // Блок заголовка появляется и сдвигается вместе с остальным экраном: следим за деревом и за размером тела.
  new MutationObserver(schedule).observe(document.documentElement, { childList: true, subtree: true });
  new ResizeObserver(schedule).observe(document.documentElement);
  schedule();
}
