import { useNavigate } from '@tanstack/react-router';
import { scopeProjectRef } from '@t3tools/client-runtime/environment';
import { EnvironmentId, ThreadId } from '@t3tools/contracts';
import { DraftId, useComposerDraftStore } from '~/composerDraftStore';
import { useScratchProject } from '~/hooks/useScratchProject';
import { newDraftId, newThreadId } from '~/lib/utils';
import { HOH_ENVIRONMENT_ID } from '../../hoh-host/config';
import {
  isSecretaryThreadId, SECRETARY_THREAD_PREFIX, type SecretaryChatInput,
} from '../../hoh-host/secretary-chat';
import { pages } from './pages';

export interface SecretaryChatEvent {
  type: 'secretaryChat';
  requestId: number;
  action: 'openChat' | 'getProposal';
  input?: SecretaryChatInput;
}

interface Binding {
  draftId: DraftId;
  threadId: ThreadId;
}

type ChatTarget = { draftId: DraftId } | { threadId: ThreadId };
const currentBindings = new Map<string, Binding>();
const environmentId = EnvironmentId.make(HOH_ENVIRONMENT_ID);
const bindingKey = (packageId: string) => `hoh:workspace-secretary:v1:${packageId}`;

function readBinding(packageId: string): Binding | undefined {
  const current = currentBindings.get(packageId);
  if (current) return current;
  try {
    const saved = JSON.parse(localStorage.getItem(bindingKey(packageId)) ?? 'null');
    if (typeof saved?.draftId === 'string' && typeof saved.threadId === 'string' && isSecretaryThreadId(saved.threadId)) {
      return { draftId: DraftId.make(saved.draftId), threadId: ThreadId.make(saved.threadId) };
    }
  } catch { /* Losing the stored chat link leaves the ordinary conversation in the sidebar. */ }
  return undefined;
}

function saveBinding(packageId: string, binding: Binding) {
  currentBindings.set(packageId, binding);
  try { localStorage.setItem(bindingKey(packageId), JSON.stringify(binding)); }
  catch { /* The current window can still reopen the conversation. */ }
}

function secretaryApi() {
  const api = window.__HOH_SECRETARY_CHAT__;
  if (!api) throw new Error('Чат секретаря недоступен.');
  return api;
}

export function useWorkspaceSecretaryChat() {
  const navigate = useNavigate();
  const scratch = useScratchProject();

  return {
    async prepare(packageId: string, input: SecretaryChatInput, isCurrent: () => boolean): Promise<ChatTarget | undefined> {
      const api = secretaryApi();
      const binding = readBinding(packageId);
      if (binding) {
        if (await api.hasThread(binding.threadId)) {
          if (!isCurrent()) return;
          api.bind(binding.threadId, input);
          return { threadId: binding.threadId };
        }
        if (!isCurrent()) return;
        const draft = useComposerDraftStore.getState().getDraftSession(binding.draftId);
        if (draft?.threadId === binding.threadId && !draft.promotedTo) {
          api.bind(binding.threadId, input);
          return { draftId: binding.draftId };
        }
      }

      const project = await scratch.openScratchProject(environmentId);
      if (!isCurrent()) return;
      if (!project) throw new Error('Не удалось открыть чат секретаря.');
      const draftId = newDraftId();
      const threadId = ThreadId.make(SECRETARY_THREAD_PREFIX + newThreadId());
      const store = useComposerDraftStore.getState();
      api.bind(threadId, input);
      // Keep the secretary away from the scratch project's ordinary draft slot.
      store.setLogicalProjectDraftThreadId(bindingKey(packageId), scopeProjectRef(project.environmentId, project.id), draftId, {
        threadId, envMode: 'local',
      });
      store.applyStickyState(draftId);
      saveBinding(packageId, { draftId, threadId });
      return { draftId };
    },
    async open(target: ChatTarget) {
      if ('threadId' in target) {
        await navigate({ to: '/$environmentId/$threadId', params: { environmentId, threadId: target.threadId } });
      } else {
        await navigate({ to: '/draft/$draftId', params: { draftId: target.draftId } });
      }
      pages.close();
    },
    async getProposal(packageId: string) {
      const binding = readBinding(packageId);
      return binding ? secretaryApi().getProposal(binding.threadId) : null;
    },
  };
}
