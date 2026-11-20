// Прослойка окна HoH вместо effect/Function (пакета effect у окна нет): только то, что нужно перенесённому коду.
export function pipe<A>(a: A): A;
export function pipe<A, B>(a: A, ab: (a: A) => B): B;
export function pipe<A, B, C>(a: A, ab: (a: A) => B, bc: (b: B) => C): C;
export function pipe<A, B, C, D>(a: A, ab: (a: A) => B, bc: (b: B) => C, cd: (c: C) => D): D;
export function pipe(a: unknown, ...fns: Array<(value: unknown) => unknown>): unknown { return fns.reduce((value, fn) => fn(value), a); }
export const identity = <A>(a: A): A => a;
