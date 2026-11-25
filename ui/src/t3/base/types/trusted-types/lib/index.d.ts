// Типы Trusted Types для dompurify (его d.ts импортирует `trusted-types/lib/index.js` из @types/trusted-types — у T3 он
// стоит как необязательная зависимость dompurify, у окна его нет; T3-DEPS.md). Объявлено ровно то, что берёт dompurify;
// подключено через typePaths манифеста (src/t3/manifest.json). Когда ведущий поставит @types/trusted-types — удалить.
export declare class TrustedHTML { private readonly brand: true }
export declare class TrustedScript { private readonly brand: true }
export declare class TrustedScriptURL { private readonly brand: true }
export interface TrustedTypePolicyOptions {
  createHTML?: ((input: string, ...args: any[]) => string) | undefined;
  createScript?: ((input: string, ...args: any[]) => string) | undefined;
  createScriptURL?: ((input: string, ...args: any[]) => string) | undefined;
}
export interface TrustedTypePolicy<Options extends TrustedTypePolicyOptions = TrustedTypePolicyOptions> {
  readonly name: string;
  createHTML(input: string, ...args: any[]): TrustedHTML;
  createScript(input: string, ...args: any[]): TrustedScript;
  createScriptURL(input: string, ...args: any[]): TrustedScriptURL;
}
export interface TrustedTypePolicyFactory {
  createPolicy<Options extends TrustedTypePolicyOptions>(policyName: string, policyOptions?: Options): TrustedTypePolicy<Options>;
  isHTML(value: unknown): value is TrustedHTML;
  isScript(value: unknown): value is TrustedScript;
  isScriptURL(value: unknown): value is TrustedScriptURL;
  readonly emptyHTML: TrustedHTML;
  readonly emptyScript: TrustedScript;
  getAttributeType(tagName: string, attribute: string, elementNs?: string, attrNs?: string): string | null;
  getPropertyType(tagName: string, property: string, elementNs?: string): string | null;
  readonly defaultPolicy: TrustedTypePolicy | null;
}
export interface TrustedTypesWindow {
  trustedTypes?: TrustedTypePolicyFactory | undefined;
  TrustedHTML: typeof TrustedHTML;
  TrustedScript: typeof TrustedScript;
  TrustedScriptURL: typeof TrustedScriptURL;
  TrustedTypePolicyFactory: TrustedTypePolicyFactory;
  TrustedTypePolicy: TrustedTypePolicy;
}
