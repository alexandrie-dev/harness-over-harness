// Прослойка окна HoH вместо effect/reactivity (T3 Code, MIT): атомов состояния T3 у окна нет; фабрики возвращают пустые значения.
export const Atom = {
  family: <K, V>(make: (key: K) => V) => (key: K): V => make(key),
  make: <V>(value: V | ((get: unknown) => V)): V => (typeof value === 'function' ? (value as (get: unknown) => V)({ self: () => ({ _tag: 'None' }) }) : value),
  withLabel: (_label: string) => <V>(atom: V): V => atom,
};
export const AsyncResult = {
  failure: <A, E>(cause: unknown): { readonly _tag: 'Failure'; readonly cause: unknown; readonly __a?: A; readonly __e?: E } => ({ _tag: 'Failure', cause }),
};
