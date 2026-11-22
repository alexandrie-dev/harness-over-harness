// Прослойка окна HoH вместо `jszip` (T3 Code, MIT): офисные и двоичные форматы и импорт тем VS Code скрыты, архивы
// не читаются (loadAsync отказывает). Типы — те, к которым обращается перенесённый код: client-runtime/zip/boundedZip.ts,
// web/openVsxThemes.ts, web/components/files/{officeArchive,extendedDocumentPreview}.ts. Наш файл, scripts/t3-sync.py его не трогает.
declare namespace JSZip {
  interface JSZipObject {
    name: string;
    dir: boolean;
    date: Date;
    async(type: string): Promise<any>;
    [key: string]: any;
  }
  interface JSZipStreamHelper<T> {
    on(event: 'data', callback: (chunk: T) => void): this;
    on(event: string, callback: (...args: any[]) => void): this;
    pause(): this;
    resume(): this;
  }
}
interface JSZip {
  files: Record<string, JSZip.JSZipObject>;
  file(path: string): JSZip.JSZipObject | null;
  [key: string]: any;
}
const JSZip: { new (): JSZip; loadAsync(data: unknown): Promise<JSZip> } = class {
  static loadAsync(): Promise<never> {
    return Promise.reject(new Error('jszip: архивы в окне HoH не читаются'));
  }
} as never;
export default JSZip;
