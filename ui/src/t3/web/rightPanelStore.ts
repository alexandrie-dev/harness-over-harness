// Прослойка окна HoH вместо apps/web/src/rightPanelStore.ts (T3 Code, MIT): у окна нет правой панели файлов,
// «открыть файл» ничего не делает.
export const useRightPanelStore = {
  getState: () => ({
    openFile: (_thread: unknown, _path: string, _line?: number): void => {},
  }),
};
