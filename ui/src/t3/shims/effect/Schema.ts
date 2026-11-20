// Прослойка окна HoH вместо effect/Schema (T3 Code, MIT): пакета effect у окна нет.
export const is = <A>(schema: { is?: (u: unknown) => boolean } | (new (...a: never[]) => A)) => (value: unknown): value is A =>
  typeof schema === 'function' ? value instanceof schema : schema.is?.(value) ?? false;
