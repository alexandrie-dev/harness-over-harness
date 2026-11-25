// Заглушки окна HoH (src/t3/base/stubs) — значения скрытых частей T3. Значение заглушки — функция; её вызов (и new)
// даёт объект, любое свойство которого — функция, возвращающая undefined (then — undefined: это не обещание; перебор —
// пустой). Так код скрытой части загружается и на уровне модуля (`export const { a, b } = make(...)`) не падает,
// но ничего не делает. Образец — пробная сборка u4 (HOH-40 веха 1). Файл пишет scripts/t3-sync.py.
/** Тип значения заглушки: его можно вызвать, создать через new, унаследовать и читать любые свойства. */
export type Anything = { (...args: any[]): any; new (...args: any[]): any; [key: string]: any };

const leaf = () => undefined;
const made: any = new Proxy({}, { get: (_target, key) => (key === 'then' ? undefined : key === Symbol.iterator ? function* () {} : leaf) });

export function absent(spec: string, name: string): Anything {
  const value = function () { return made; };
  Object.defineProperty(value, 'name', { value: `${spec}:${name}` });
  return value as unknown as Anything;
}
