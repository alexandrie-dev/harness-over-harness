// Замена окна HoH вместо apps/web/src/components/T3Wordmark.tsx (T3 Code, MIT): знак T3 не берём — на его месте
// эмблема HoH (src/assets/emblem-48.png). Имя и подписи те же, что у T3: вызывающий код T3 не меняется.
import type { SVGProps } from "react";
import emblem from "../../../../assets/emblem-48.png";

export function T3Wordmark({ "aria-label": label, ...props }: SVGProps<SVGSVGElement>) {
  return (
    <svg {...props} aria-label={label === undefined ? undefined : "HoH"} viewBox="0 0 48 48" xmlns="http://www.w3.org/2000/svg">
      <image href={emblem} width="48" height="48" />
    </svg>
  );
}
