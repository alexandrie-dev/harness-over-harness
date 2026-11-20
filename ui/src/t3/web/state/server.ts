// Прослойка окна HoH вместо apps/web/src/state/server.ts (T3 Code, MIT): окружений хоста T3 в окне нет; любое обращение возвращает пустое значение.
import { makeStub } from '../../shims/stub';
export const serverEnvironment = makeStub();
