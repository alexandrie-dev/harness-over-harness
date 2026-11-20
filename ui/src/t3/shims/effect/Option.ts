// Прослойка окна HoH вместо effect/Option (T3 Code, MIT): пакета effect у окна нет.
export type Option<A> = { readonly _tag: 'None' } | { readonly _tag: 'Some'; readonly value: A };
export const none = <A = never>(): Option<A> => ({ _tag: 'None' });
export const some = <A>(value: A): Option<A> => ({ _tag: 'Some', value });
export const getOrNull = <A>(option: Option<A>): A | null => (option._tag === 'Some' ? option.value : null);
