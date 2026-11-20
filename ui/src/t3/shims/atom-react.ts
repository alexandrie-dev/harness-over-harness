// Прослойка окна HoH вместо @effect/atom-react (T3 Code, MIT): атомов состояния T3 у окна нет.
export function useAtomValue<V>(atom: V): V { return atom; }
