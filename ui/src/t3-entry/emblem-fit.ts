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

// Приветствие главной (hoh-screens/HomeBody.tsx) стоит в начале прокручиваемого тела; эмблема над ним добавляет отступ HOME_EXTRA.
// Она показывается, только если с отступом всё тело помещается без прокрутки; иначе скрыта (не накладывается на карточки).
const HOME_EXTRA = 126;

// Корешок (u4, --hoh-notch-height) несёт эмблему сверху по центру: большая у заголовка не повторяется.
function notchShown(): boolean {
  return (parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--hoh-notch-height')) || 0) > 0;
}

function fit(): void {
  if (notchShown()) {
    for (const mark of document.querySelectorAll('[data-hoh-emblem]')) mark.removeAttribute('data-hoh-emblem');
    return;
  }
  const top = headerBottom();
  const home = document.querySelector('[data-hoh-home-body]');
  for (const heading of document.querySelectorAll<HTMLElement>(HEADINGS)) {
    // Заголовок пустого разговора при главной не рисует эмблему: она у приветствия.
    heading.toggleAttribute('data-hoh-emblem', !home && heading.getBoundingClientRect().top - top >= NEED);
  }
  for (const head of document.querySelectorAll<HTMLElement>('.hoh-home__head')) {
    const body = head.closest<HTMLElement>('[data-hoh-home-body]');
    const inner = body?.firstElementChild as HTMLElement | null | undefined;
    if (!body || !inner) continue;
    // Высота содержимого (колонка карточек и «Needs your answer» рядом или друг под другом), а не растянутой обёртки.
    const style = getComputedStyle(inner);
    const heights = [...inner.children].map(child => (child as HTMLElement).offsetHeight);
    const content = style.flexDirection.startsWith('column') ? heights.reduce((a, b) => a + b, 0) + (parseFloat(style.rowGap) || 0) * Math.max(0, heights.length - 1) : Math.max(0, ...heights);
    const padding = (parseFloat(style.paddingTop) || 0) + (parseFloat(style.paddingBottom) || 0) + (parseFloat(getComputedStyle(body).paddingBottom) || 0);
    const natural = content + padding - (head.hasAttribute('data-hoh-emblem') ? HOME_EXTRA : 0);
    head.toggleAttribute('data-hoh-emblem', natural + HOME_EXTRA <= body.clientHeight);
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
