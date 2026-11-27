import { Channel, invoke } from '@tauri-apps/api/core';
import { ArrowLeftIcon } from 'lucide-react';
import { useLayoutEffect, useRef, useState } from 'react';
import { externalUiError, type PagePlugin } from './external-ui-sdk';
import { pages } from './pages';
import { useWorkspaceSecretaryChat, type SecretaryChatEvent } from './workspace-secretary-chat';

// Serialize open, layout and hide across page changes. A package read can outlive its React page.
let pendingWorkspaceUpdate = Promise.resolve();
let workspaceSequence = 0;
function updateWorkspace(operation: () => Promise<void> | undefined) {
  const update = pendingWorkspaceUpdate.then(operation);
  pendingWorkspaceUpdate = update.catch(() => {});
  return update;
}

function coveredByPopup(area: DOMRect) {
  const popups = document.querySelectorAll<HTMLElement>(
    'dialog[open], [role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"], [data-slot="popover-popup"]',
  );
  return Array.from(popups).some(popup => {
    if (popup.closest('[aria-hidden="true"], [hidden], [data-closed]')) return false;
    const rect = popup.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0 &&
      rect.left < area.right && rect.right > area.left && rect.top < area.bottom && rect.bottom > area.top;
  });
}

export function ExternalUiWorkspace({ item }: { item: PagePlugin }) {
  const surface = useRef<HTMLDivElement>(null);
  const [error, setError] = useState('');
  const [attempt, setAttempt] = useState(0);
  const { id, digest, name, version, workspaceLabel, secretaryChat } = item;
  const secretary = useWorkspaceSecretaryChat();
  const secretaryRef = useRef(secretary);
  useLayoutEffect(() => { secretaryRef.current = secretary; }, [secretary]);

  useLayoutEffect(() => {
    const target = surface.current;
    if (!target) return;
    let active = true;
    let frame = 0;
    let previous = '';
    setError('');

    const onError = (failure: unknown) => {
      if (active) setError(externalUiError(failure, 'The Workspace page could not be opened.'));
    };

    const measure = () => {
      frame = 0;
      const rect = target.getBoundingClientRect();
      const x = Math.max(0, rect.left);
      const y = Math.max(0, rect.top);
      const width = Math.min(window.innerWidth, rect.right) - x;
      const height = Math.min(window.innerHeight, rect.bottom) - y;
      const bounds = width > 0 && height > 0 && !coveredByPopup(rect) ? { x, y, width, height } : null;
      const key = JSON.stringify([bounds, window.devicePixelRatio]);
      if (key === previous) return;
      previous = key;
      void updateWorkspace(() => {
        if (active) return invoke('layout_external_ui_workspace', { id, bounds });
      }).catch(onError);
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(measure);
    };
    const resize = new ResizeObserver(schedule);
    const mutation = new MutationObserver(schedule);
    const workspaceId = secretaryChat ? ++workspaceSequence : undefined;
    const channel = workspaceId ? new Channel<SecretaryChatEvent>() : undefined;
    if (channel) channel.onmessage = event => {
      if (!active || event.type !== 'secretaryChat') return;
      const chat = secretaryRef.current;
      const reply = (result: { value: string | null } | { error: string }) => {
        if (active) return invoke('reply_external_ui_secretary', {
          workspaceId, requestId: event.requestId, ...result,
        });
      };
      void (async () => {
        if (event.action === 'getProposal') {
          await reply({ value: await chat.getProposal(id) });
          return;
        }
        if (!event.input) throw new Error('Не передан контекст кабинета.');
        const target = await chat.prepare(id, event.input, () => active);
        if (!target) return;
        await reply({ value: null });
        if (active) await chat.open(target);
      })().catch(failure => {
        void reply({ error: externalUiError(failure, 'Не удалось открыть чат секретаря.') })
          ?.catch(onError);
      });
    };

    void updateWorkspace(() => {
      if (active) return invoke('open_external_ui_plugin', {
        package: { id, digest, name, version, workspaceLabel, ...(secretaryChat ? { secretaryChat } : {}) },
        ...(channel ? { workspaceId, onEvent: channel } : {}),
      });
    }).then(() => {
      if (!active) return;
      resize.observe(target);
      window.addEventListener('resize', schedule);
      mutation.observe(document.body, {
        subtree: true, childList: true, attributes: true,
        attributeFilter: ['open', 'aria-hidden', 'hidden', 'data-open', 'data-closed', 'data-state'],
      });
      schedule();
    }, onError);

    return () => {
      active = false;
      cancelAnimationFrame(frame);
      resize.disconnect();
      mutation.disconnect();
      window.removeEventListener('resize', schedule);
      void updateWorkspace(() => invoke('layout_external_ui_workspace', { id, bounds: null })).catch(console.error);
    };
  }, [id, digest, name, version, workspaceLabel, secretaryChat, attempt]);

  return (
    <section className="hoh-page hoh-workspace-page" aria-label={workspaceLabel}>
      <header className="hoh-workspace-page__bar">
        <button type="button" onClick={() => pages.close()} aria-label={`Close ${workspaceLabel}`}>
          <ArrowLeftIcon aria-hidden size={16} />
        </button>
        <h1>{workspaceLabel}</h1>
      </header>
      <div ref={surface} className="hoh-workspace-page__surface">
        {error && <div role="alert" className="hoh-workspace-page__error">
          <p>{error}</p>
          <button type="button" onClick={() => setAttempt(value => value + 1)}>Retry</button>
        </div>}
      </div>
    </section>
  );
}
