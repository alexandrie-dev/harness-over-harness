// The engine as the Tauri host exposes it to the main window (apps/hoh-desktop/src/native.rs):
// the same commands and events the current window uses, plus `approval_decide` for permission
// requests answered in the chat.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { EngineNotification, EngineQuestion, EngineStatus, HohEngine } from './engine.ts';

export function tauriEngine(): HohEngine {
  return {
    request: (method, params) => invoke('engine_request', { method, params }),
    subscribe: listener => listen<EngineNotification>('engine-notification', event => listener(event.payload)),
    subscribeStatus: listener => listen<EngineStatus>('engine-status', event => listener(event.payload)),
    subscribeQuestions: listener => listen<EngineQuestion>('engine-question', event => listener(event.payload)),
    answerQuestion: (requestId, answer) => invoke<void>('question_answer', { requestId, answer }),
    decideApproval: (approvalId, outcome) => invoke<void>('approval_decide', { approvalId, outcome }),
  };
}
