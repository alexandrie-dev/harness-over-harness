import type { ChatEntry } from './projection.mjs';
import type { ChatTransport, PendingQuestion, QuestionAnswer } from './transport';
export type ChatData = {
  entries: ChatEntry[];
  connection: 'connecting' | 'ready' | 'recovering' | 'error';
  busy: boolean; canceling: boolean; error: string;
  privacy: 'local' | 'remote' | 'unknown'; folderLabel: string; draft: string;
  questions: PendingQuestion[];
};
export type ChatState = ChatData & {
  setDraft(value: string): void;
  connect(): Promise<void>;
  disconnect(): void;
  send(): Promise<void>;
  stop(): Promise<void>;
  answerQuestion(requestId: string, answer: QuestionAnswer): Promise<void>;
  dismissQuestion(requestId: string): Promise<void>;
};
export class ChatController {
  constructor(transportFactory: () => ChatTransport, changed?: (state: ChatData) => void, frame?: () => Promise<void>);
  state: ChatData;
  setDraft(value: string): void;
  connect(): Promise<void>;
  disconnect(): void;
  send(): Promise<void>;
  stop(): Promise<void>;
  answerQuestion(requestId: string, answer: QuestionAnswer): Promise<void>;
  dismissQuestion(requestId: string): Promise<void>;
}
