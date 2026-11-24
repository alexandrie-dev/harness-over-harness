// Единственное место, где окно подтверждения говорит с ядром Tauri.
// Права метки `approval` включают одну команду `approval_decide` (docs/window.md §4, п.3).
// Окно не видит ни ключа, ни mac: подпись ставит ядро по записи, которую оно само показало.
import { invoke } from '@tauri-apps/api/core';
import type { ApprovalOutcome, Risk } from '../../protocol';

/** То, что ядро показывает пользователю. Поля, кроме `details`, покрыты mac на стороне ядра; `details` только для показа. */
export interface ApprovalView {
  approvalId: string;
  /** `unassessed`: the engine could not judge the action (a request of Claude); the window says so and does not paint it red. */
  risk: Risk | 'unassessed';
  /** Текст на языке пользователя, собран движком из структуры действия. */
  humanText: string;
  folderTrust?: boolean;
  /** Объяснение, написанное моделью. Недоверенный текст: только показывать. */
  reason?: string;
  details?: string;
  /** Конец срока, мс от начала эпохи. */
  expiresAt: number;
}

/** Решение, которое может принять окно. «cancelled» и «unavailable» ставит только движок. */
export type WindowDecision = Extract<ApprovalOutcome, 'allowed-once' | 'rejected'>;

interface ApprovalHost {
  current(): Promise<ApprovalView>;
  decide(approvalId: string, outcome: WindowDecision): Promise<void>;
}

// Проверки подставляют ядро через window.__HOH_APPROVAL__ (вне Tauri).
declare global { interface Window { __HOH_APPROVAL__?: ApprovalHost; __TAURI_INTERNALS__?: unknown } }

// Имена и формы команд закреплены в docs/window.md §4; реализация — пакет 4.
// `approval_decide` названа в docs/window.md §4.
const tauri: ApprovalHost = {
  current: () => invoke<ApprovalView>('approval_current'),
  decide: (approvalId, outcome) => invoke<void>('approval_decide', { approvalId, outcome }),
};

export function approvalHost(): ApprovalHost {
  if (window.__HOH_APPROVAL__) return window.__HOH_APPROVAL__;
  if (window.__TAURI_INTERNALS__) return tauri;
  throw new Error('нет связи с ядром');
}
