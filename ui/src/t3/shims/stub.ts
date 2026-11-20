// Заглушка для объектов хоста T3, которых в окне HoH нет: любое обращение возвращает функцию, дающую undefined.
export const makeStub = (): any => new Proxy({}, { get: () => () => undefined });
