// The engine as the Tauri host exposes it to the main window (apps/hoh-desktop/src/native.rs):
// the same commands and events the current window uses, plus `approval_decide` for permission
// requests answered in the chat.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { subscribeEngineNotifications } from '../lib/engine-notifications.ts';
import type { EngineQuestion, EngineStatus, FolderListing, HohEngine } from './engine.ts';

export function tauriEngine(): HohEngine {
  return {
    request: (method, params) => invoke('engine_request', { method, params }),
    async environmentPlatform() {
      const { os, arch } = await invoke<{ os: string; arch: string }>('environment_platform');
      return {
        os: os === 'win32' || os === 'windows' ? 'windows' : os === 'darwin' || os === 'linux' ? os : 'unknown',
        arch: arch === 'x64' || arch === 'arm64' ? arch : 'other',
      };
    },
    projectGit: input => invoke('project_git', input),
    confirmExternalImage: input => invoke('confirm_external_image', input),
    readImageAsset: input => invoke('read_image_asset', { sessionId: input.sessionId, path: input.path }),
    subscribe: subscribeEngineNotifications,
    subscribeStatus: listener => listen<EngineStatus>('engine-status', event => listener(event.payload)),
    subscribeQuestions: listener => listen<EngineQuestion>('engine-question', event => listener(event.payload)),
    answerQuestion: (requestId, answer) => invoke<void>('question_answer', { requestId, answer }),
    answerSecret: (sessionId, requestId, secret) => invoke<void>('question_secret_answer', { sessionId, requestId, secret }),
    decideApproval: (approvalId, outcome) => invoke<void>('approval_decide', { approvalId, outcome }),
    // `browse_folders` of the Tauri host (apps/hoh-desktop, u4): T3's folder list for "Add project".
    browseFolders: input => invoke<FolderListing>('browse_folders', { partialPath: input.partialPath, cwd: input.cwd ?? null }),
    listProjectEntries: input => invoke('project_list_entries', input),
    readProjectFile: input => invoke('project_read_file', input),
    availableProjectEditors: () => invoke('project_available_editors'),
    openProjectEditor: input => invoke('project_open_in_editor', input),
  };
}
