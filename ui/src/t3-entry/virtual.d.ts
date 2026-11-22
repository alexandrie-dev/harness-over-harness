// Модули, которые подставляет сборка входа T3 (src/t3-entry/vite.ts).
declare module 'virtual:hoh-host' {
  export const installHohHost: import('./host').InstallHohHost;
}
// apps/web/src/bootstrap.ts T3 как есть: побочный эффект — запуск клиента.
declare module 'virtual:t3-bootstrap' {}
