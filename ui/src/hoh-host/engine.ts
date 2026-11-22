// What the HoH host needs from the engine: the same methods and notifications the current window
// uses (ui/src/screens/chat/transport.ts), reached through the Tauri bridge (tauri.ts) or a fake in tests.

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
  }>;
}
export type EngineQuestionAnswer =
  | { readonly answers: ReadonlyArray<{ readonly id: string; readonly selected: string[]; readonly custom?: string }> }
  | { readonly cancelled: true };

/** What the person decided about a permission request. The engine alone sets the other outcomes. */
export type ApprovalDecision = 'allowed-once' | 'rejected';

export interface HohEngine {
  request(method: string, params: unknown): Promise<any>;
  subscribe(listener: (notification: EngineNotification) => void): Promise<Unsubscribe>;
  subscribeStatus?(listener: (status: EngineStatus) => void): Promise<Unsubscribe>;
  subscribeQuestions?(listener: (question: EngineQuestion) => void): Promise<Unsubscribe>;
  answerQuestion?(requestId: string, answer: EngineQuestionAnswer): Promise<void>;
  /** The permission request of the assistant, answered in the chat. */
  decideApproval?(approvalId: string, decision: ApprovalDecision): Promise<void>;
}

/** `hoh/executor/access/get` (the engine's contract): the mode asked for and the one in effect. */
export type AccessMode = 'confirm' | 'full';
