import type { ChatEntry } from './projection.mjs';
import type { DiscoverResult } from '../../lib/executor.mjs';
import type { ChatTransport, PendingQuestion, QuestionAnswer } from './transport';
export type ChatData = {
  entries: ChatEntry[];
  connection: 'connecting' | 'ready' | 'recovering' | 'error';
  openingChat: boolean; busy: boolean; canceling: boolean; error: string;
  privacy: 'local' | 'remote' | 'unknown'; folderLabel: string; model: string; modelIcon: string; modelScope: string; permissions: string; notice: string; folderPath: string;
  /** Mode of the NEXT turn of the open chat as the engine reports it (`session.mode` of access/get with the chat's id); '' = this chat does not run through Claude. */
  access: '' | 'confirm' | 'full'; accessBusy: boolean; /** The mode waits for the next message: the live process of the chat still runs in the other one (`session.pending`). */ accessPending: boolean; /** The engine refused to change the folder of this chat (it runs through Claude): the screen offers a new chat. */ folderFixed: boolean; draft: string;
  /** The executor of the OPEN chat (`session` of access/get): 'claude' or '' (the engine's own model) and its model id; `newExecutor`/`newExecutorModel` are the choice for NEW chats (`selected`). */
  executor: '' | 'claude'; executorModel: string; newExecutor: '' | 'claude'; newExecutorModel: string; /** The engine ran Claude without the settings of the project folder (`--setting-sources` without `project`). */ projectSettingsOff: boolean; /** The engine ran Claude with the person's own settings (`--setting-sources` names `user`). */ personalSettings: boolean; executorModels: { id: string; name: string }[]; executorBusy: boolean;
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
