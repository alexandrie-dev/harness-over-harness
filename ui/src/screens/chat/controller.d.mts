import type { ChatEntry } from './projection.mjs';
import type { DiscoverResult } from '../../lib/executor.mjs';
import type { ChatTransport, PendingQuestion, QuestionAnswer } from './transport';
export type ChatData = {
  entries: ChatEntry[];
  connection: 'connecting' | 'ready' | 'recovering' | 'error';
  busy: boolean; canceling: boolean; error: string;
  privacy: 'local' | 'remote' | 'unknown'; folderLabel: string; model: string; modelIcon: string; modelScope: string; permissions: string; notice: string; folderPath: string;
  /** Mode of the Claude executor as the engine reports it (effective); '' = the engine has no such setting. */
  access: '' | 'confirm' | 'full'; accessBusy: boolean; draft: string;
  /** The executor the engine runs new chats with (`selected` of access/get): 'claude' or '' (the engine's own model), its model id and the models last discovered. */
  executor: '' | 'claude'; executorModel: string; executorModels: { id: string; name: string }[]; executorBusy: boolean;
  questions: PendingQuestion[];
  /** Chats of the engine, newest first; the open one is always present. */
  sessions: ChatSessionRow[]; currentId: string; working: string[];
};
export type ChatSessionRow = { id: string; title: string; updatedAt: number };
export type ChatState = ChatData & {
  setDraft(value: string): void;
  connect(): Promise<void>;
  disconnect(): void;
  send(): Promise<void>;
  stop(): Promise<void>;
  openSession(id: string): Promise<void>;
  newChat(): Promise<void>;
  setFolder(path: string): Promise<void>;
  openLink(kind: 'url' | 'file' | 'reveal', target: string): Promise<void>;
  refuseLink(): void;
  setAccess(mode: 'confirm' | 'full'): Promise<void>;
  discoverClaude(): Promise<DiscoverResult>;
  loadExecutorModels(): Promise<void>;
  selectExecutor(executor: 'claude' | null, model: string): Promise<boolean>;
  blockedLink(): void;
  dismissNotice(): void;
  answerQuestion(requestId: string, answer: QuestionAnswer): Promise<void>;
  dismissQuestion(requestId: string): Promise<void>;
};
export class ChatController {
  constructor(transportFactory: () => ChatTransport, changed?: (state: ChatData) => void, frame?: () => Promise<void>);
  state: ChatData;
  setDraft(value: string): void;
  connect(recovering?: boolean, target?: string): Promise<void>;
  disconnect(): void;
  send(): Promise<void>;
  stop(): Promise<void>;
  openSession(id: string): Promise<void>;
  newChat(): Promise<void>;
  setFolder(path: string): Promise<void>;
  openLink(kind: 'url' | 'file' | 'reveal', target: string): Promise<void>;
  refuseLink(): void;
  setAccess(mode: 'confirm' | 'full'): Promise<void>;
  discoverClaude(): Promise<DiscoverResult>;
  loadExecutorModels(): Promise<void>;
  selectExecutor(executor: 'claude' | null, model: string): Promise<boolean>;
  blockedLink(): void;
  dismissNotice(): void;
  answerQuestion(requestId: string, answer: QuestionAnswer): Promise<void>;
  dismissQuestion(requestId: string): Promise<void>;
  sessionId: string;
}
