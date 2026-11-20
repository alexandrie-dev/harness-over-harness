// Прослойка окна HoH вместо effect/Cause (T3 Code, MIT): пакета effect у окна нет.
export const fail = <E>(error: E): { readonly _tag: 'Fail'; readonly error: E } => ({ _tag: 'Fail', error });
