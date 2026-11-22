// Замена окна HoH вместо apps/web/src/components/T3AppIcon.tsx (T3 Code, MIT): значок приложения T3 не берём —
// на его месте эмблема HoH (src/assets/emblem-48.png).
import appIconUrl from "../../../../assets/emblem-48.png";

export function T3AppIcon(props: { readonly className?: string }) {
  return <img src={appIconUrl} alt="" className={props.className} />;
}
