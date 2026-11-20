// Прослойка окна HoH вместо @t3tools/contracts (T3 Code, MIT): только те типы и значения,
// которые нужны перенесённому коду. Протокол окна — ui/src/protocol.ts, это отдельная вещь.
export type EnvironmentId = string;
export interface ContextMenuItem<Action extends string = string> {
  readonly id: Action;
  readonly label: string;
  readonly destructive?: boolean;
  readonly disabled?: boolean;
}
export const SOURCE_CONTROL_MEDIA_ASSET = 'source-control-media' as const;
/** Ресурсы-вложения сервера T3. В окне HoH таких ресурсов нет: изображения приходят адресом или данными. */
export type AssetResource =
  | { readonly _tag: 'attachment'; readonly id: string }
  | { readonly _tag: 'workspace-file'; readonly threadId: string; readonly path: string }
  | { readonly _tag: 'media-file'; readonly threadId: string; readonly path: string }
  | { readonly _tag: typeof SOURCE_CONTROL_MEDIA_ASSET; readonly url: string }
  | { readonly _tag: 'tool-output-image'; readonly id: string };
export const AuthFilesystemReadScope = 'filesystem:read' as const;
export const AuthOrchestrationOperateScope = 'orchestration:operate' as const;
export const AuthPreviewOperateScope = 'preview:operate' as const;
// В окне HoH нет внешних редакторов: ссылки вида vscode://file/... не открываются, поэтому список схем пуст.
export const LOCAL_FILE_LINK_SCHEMES: ReadonlyArray<string> = [];
export const isLocalEditorFileUrl = (_url: URL): boolean => false;
export type EditorId = string;
export type ServerConfig = Record<string, unknown>;
export type ScopedThreadRef = { readonly environmentId: EnvironmentId; readonly threadId: string };
export type ServerProviderSkill = Record<string, unknown>;
export type ThreadPullRequestKey = string;
export class AssetWorkspaceAssetNotFoundError extends Error {}

/** Режимы доступа T3. Окно показывает два: «approval-required» (с подтверждением) и «full-access» (полный доступ). */
export type RuntimeMode = 'approval-required' | 'auto-accept-edits' | 'auto' | 'full-access';
export type ManagedPolicySummary = { readonly organization: string; readonly limits?: { readonly maxRuntimeMode?: RuntimeMode } };
