import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { MethodMap, Notification } from '../../protocol';
import type { DiscoverResult } from '../../lib/executor.mjs';

export type ChatMethod = Extract<keyof MethodMap,
  'hoh/employee/open' | 'hoh/security/summary' | 'hoh/settings/get' | 'session/resume' | 'session/events' |
  'session/working-directory/get' | 'session/prompt' | 'session/cancel' | 'session/list' | 'session/working-directory/set'>;
export type EngineStatus = { state: 'starting' | 'ready' | 'unavailable' | 'restarting' | 'stopped' };
// Shapes of `user-question/ask` (crates/hoh-protocol/src/sdk.rs); the native host checks every answer against the question.
export interface AskedQuestion {
  id: string; question: string; detail?: string; header?: string;
  options?: { label: string; description?: string }[]; multiSelect?: boolean;
}
export interface PendingQuestion { requestId: string; sessionId: string; questions: AskedQuestion[] }
export type QuestionAnswer = { answers: { id: string; selected: string[]; custom?: string }[] } | { cancelled: true };
export interface ApprovalRecord {
  at: number; asker: 'assistant' | 'you'; method: string; humanText: string;
  outcome: string; reason?: string; details?: string;
}
export type AccessMode = 'confirm' | 'full';
/** `hoh/executor/access/get` (the engine's contract; not in the generated protocol yet): the mode asked for and the one in effect. */
/** With a `sessionId` the engine also answers for that chat (`session`): its executor, model, where its data goes, the mode of its next turn, the mode of its live process and whether the change waits for the next message. */
export interface AccessSession { executor: 'claude' | null; model: string | null; privacy: 'local' | 'remote' | null; mode: AccessMode | null; running: AccessMode | null; pending: boolean }
export interface AccessState { mode: AccessMode; effective: { mode: AccessMode; flags: string[] }; selected: { executor: 'claude'; model: string } | null; session?: AccessSession }
export interface ChatTransport {
  request<M extends ChatMethod>(method: M, params: MethodMap[M]['params']): Promise<MethodMap[M]['result']>;
  request(method: 'hoh/executor/access/get', params: { executor: 'claude'; sessionId?: string }): Promise<AccessState>;
  /** Privileged: the host asks the person in the guard window before the engine applies it. */
  request(method: 'hoh/executor/access/set', params: { executor: 'claude'; mode: AccessMode }): Promise<unknown>;
  /** Read-only, a real check every time: is `claude` there, signed in, which models. */
  request(method: 'hoh/executor/discover', params: { executor: 'claude' }): Promise<DiscoverResult>;
  /** Privileged: new chats run through Claude with this model (`executor: null`: back on the engine's own model). */
  request(method: 'hoh/executor/select', params: { executor: 'claude' | null; model: string }): Promise<unknown>;
  subscribe(listener: (event: Notification) => void): Promise<() => void>;
  subscribeStatus(listener: (status: EngineStatus) => void): Promise<() => void>;
  subscribeQuestions?(listener: (question: PendingQuestion) => void): Promise<() => void>;
  answerQuestion?(requestId: string, answer: QuestionAnswer): Promise<void>;
  approvalHistory?(): Promise<ApprovalRecord[]>;
  /** Open a link of an answer through the native host (it checks the address or the path itself). */
  openLink?(kind: 'url' | 'file' | 'reveal', target: string, sessionId: string): Promise<void>;
}
declare global {
  interface Window { __HOH_CHAT_TEST_TRANSPORT__?: ChatTransport; }
}

export function chatTransport(): ChatTransport {
  // Browser-only adapter, installed by the headless harness; never used in Tauri.
  if (!isTauri() && window.__HOH_CHAT_TEST_TRANSPORT__) return window.__HOH_CHAT_TEST_TRANSPORT__;
  // Native bridge delivers protocol frames and process lifecycle separately.
  return {
    request: ((method: string, params: unknown) => invoke('engine_request', { method, params })) as ChatTransport['request'],
    subscribe: listener => listen<Notification>('engine-notification', event => listener(event.payload)),
    subscribeStatus: listener => listen<EngineStatus>('engine-status', event => listener(event.payload)),
    subscribeQuestions: listener => listen<PendingQuestion>('engine-question', event => listener(event.payload)),
    answerQuestion: (requestId, answer) => invoke<void>('question_answer', { requestId, answer }),
    approvalHistory: () => invoke<ApprovalRecord[]>('approval_history'),
    openLink: (kind, target, sessionId) => invoke<void>('open_link', { kind, target, sessionId }),
  };
}
