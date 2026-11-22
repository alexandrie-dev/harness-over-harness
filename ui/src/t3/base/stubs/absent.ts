// Заглушки окна HoH (src/t3/base/stubs) — значения скрытых частей T3: вызов даёт undefined, new — пустой объект,
// любое свойство — undefined. Код скрытой части загружается, но ничего не делает. Для видимых импортёров (T3-DEPS.md,
// «Видимые импортёры») в э3 — рабочие заглушки вместо этих. Файл пишет scripts/t3-sync.py.
/** Тип значения заглушки: его можно вызвать, создать через new, унаследовать и читать любые свойства. */
export type Anything = { (...args: any[]): any; new (...args: any[]): any; [key: string]: any };

export function absent(spec: string, name: string): Anything {
  const value = function () { return undefined; };
  Object.defineProperty(value, 'name', { value: `${spec}:${name}` });
  return value as unknown as Anything;
}
