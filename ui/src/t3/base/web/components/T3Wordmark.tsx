// Замена окна HoH вместо apps/web/src/components/T3Wordmark.tsx (T3 Code, MIT): знак T3 не берём — на его месте
// эмблема HoH (прозрачный растр из src/assets/emblem-a.webp, scripts/emblem-assets.sh). Имя и подписи те же, что у T3:
// вызывающий код T3 не меняется (SidebarChrome.tsx зовёт <T3Wordmark aria-label className="h-[1cap] w-auto">).
// Тонкие линии пирамиды при 26 px сливаются, поэтому знак показывается в поле 32×34 px, а растры 1×/1.5×/2×/3× отдаёт srcset:
// браузер берёт растр под плотность экрана (масштаб окна 100 %, 150 % и т.д.) и не масштабирует его сам.
import type { SVGProps } from "react";
import brand1 from "../../../../assets/emblem-brand-1x.png";
import brand15 from "../../../../assets/emblem-brand-15x.png";
import brand2 from "../../../../assets/emblem-brand-2x.png";
import brand3 from "../../../../assets/emblem-brand-3x.png";

export function T3Wordmark({ "aria-label": label, className }: SVGProps<SVGSVGElement>) {
  return (
    <img
      className={className}
      src={brand1}
      srcSet={`${brand1} 1x, ${brand15} 1.5x, ${brand2} 2x, ${brand3} 3x`}
      alt={label === undefined ? "" : "HoH"}
      width={32}
      height={34}
      style={{ width: 32, height: 34 }}
      draggable={false}
    />
  );
}
