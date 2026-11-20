import type { ChatEntry } from './projection.mjs';
import type { ChatTransport, PendingQuestion, QuestionAnswer } from './transport';
export type ChatData = {
  entries: ChatEntry[];
  connection: 'connecting' | 'ready' | 'recovering' | 'error';
  busy: boolean; canceling: boolean; error: string;
  privacy: 'local' | 'remote' | 'unknown'; folderLabel: string; model: string; modelIcon: string; modelScope: string; permissions: string; notice: string; folderPath: string; draft: string;
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
  blockedLink(): void;
  dismissNotice(): void;
  answerQuestion(requestId: string, answer: QuestionAnswer): Promise<void>;
  dismissQuestion(requestId: string): Promise<void>;
  sessionId: string;
}
