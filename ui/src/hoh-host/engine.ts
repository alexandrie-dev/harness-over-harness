// What the HoH host needs from the engine: the same methods and notifications the current window
// uses (ui/src/screens/chat/transport.ts), reached through the Tauri bridge (tauri.ts) or a fake in tests.
import type { EnvironmentPlatform } from './config.ts';

import type { EditorId, ProjectListEntriesResult, ProjectReadFileResult } from '@t3tools/contracts';
import type { WorkspaceGitQuery } from './workspace-git.ts';

export interface EngineNotification { readonly method: string; readonly params: any }
export type EngineStatus = { readonly state: 'starting' | 'ready' | 'unavailable' | 'restarting' | 'stopped' };
export type Unsubscribe = () => void;

/** `user-question/ask` (crates/hoh-protocol/src/sdk.rs), as the native host forwards it. */
export interface EngineQuestion {
  readonly requestId: string;
  readonly sessionId: string;
  readonly questions: ReadonlyArray<{
    readonly id: string; readonly question: string; readonly detail?: string; readonly header?: string;
    readonly options?: ReadonlyArray<{ readonly label: string; readonly description?: string }>;
    readonly multiSelect?: boolean;
    readonly intent?: { readonly kind: string };
  }>;
}
export type EngineQuestionAnswer =
  | { readonly answers: ReadonlyArray<{ readonly id: string; readonly selected: string[]; readonly custom?: string }> }
  | { readonly cancelled: true };

/** What the person decided about a permission request. The engine alone sets the other outcomes. */
export type ApprovalDecision = 'allowed-once' | 'rejected';

export interface HohEngine {
  projectGit?(input: { cwd: string; query: WorkspaceGitQuery }): Promise<unknown>;
  request(method: string, params: unknown): Promise<any>;
  environmentPlatform?(): Promise<EnvironmentPlatform>;
  /** Native-only bounded read of a raster, rooted by the native host in this conversation's cwd. */
  readImageAsset?(input: { readonly sessionId: string; readonly path: string }): Promise<LocalImageAsset>;
  confirmExternalImage?(input: { readonly sessionId: string; readonly path: string }): Promise<LocalImageAsset | null>;
  subscribe(listener: (notification: EngineNotification) => void): Promise<Unsubscribe>;
  subscribeStatus?(listener: (status: EngineStatus) => void): Promise<Unsubscribe>;
  subscribeQuestions?(listener: (question: EngineQuestion) => void): Promise<Unsubscribe>;
  answerQuestion?(requestId: string, answer: EngineQuestionAnswer): Promise<void>;
  answerSecret?(sessionId: string, requestId: string, secret: string | null): Promise<void>;
  /** The permission request of the assistant, answered in the chat. */
  decideApproval?(approvalId: string, decision: ApprovalDecision): Promise<void>;
  /**
   * Folders under a typed path, for T3's "Add project" (its `filesystem.browse`, apps/server
   * WorkspaceEntries.browse): `~/` lists the home folder, `~/Pro` the folders of home starting with
   * "Pro". Only folder names; the window sees no files. Without it, a project is added by its full path.
   */
  browseFolders?(input: FolderBrowseInput): Promise<FolderListing>;
  listProjectEntries?(input: { readonly cwd: string; readonly directoryPath: string }): Promise<ProjectListEntriesResult>;
  readProjectFile?(input: { readonly cwd: string; readonly path: string; readonly allowMissing: boolean }): Promise<ProjectReadFileResult>;
  availableProjectEditors?(): Promise<readonly EditorId[]>;
  openProjectEditor?(input: {
    readonly cwd: string;
    readonly path: string;
    readonly editor: EditorId;
    readonly position?: { readonly line?: number; readonly column?: number };
  }): Promise<void>;
}

export interface LocalImageAsset {
  readonly dataBase64: string;
  readonly mimeType: 'image/gif' | 'image/jpeg' | 'image/png' | 'image/webp';
  readonly sizeBytes: number;
  readonly sourcePath: string;
  readonly fileRevision: string;
  readonly decodeProfile: {
    readonly canvasPixels: number;
    readonly decodedFramePixels: number;
    readonly animationPixelsPerSecond: number;
    readonly frameCount: number;
  };
}

export interface FolderBrowseInput { readonly partialPath: string; readonly cwd?: string }
export interface FolderListing {
  readonly parentPath: string;
  readonly entries: ReadonlyArray<{ readonly name: string; readonly fullPath: string }>;
}

/** `hoh/executor/access/get` (the engine's contract): the mode asked for and the one in effect. */
export type AccessMode = 'confirm' | 'auto-accept-edits' | 'auto' | 'full';
