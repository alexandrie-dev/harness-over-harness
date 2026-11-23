import { scopeThreadRef } from '@t3tools/client-runtime/environment';
import { useNavigate } from '@tanstack/react-router';
import { ArrowUpRightIcon } from 'lucide-react';
import { useCallback, useMemo, useSyncExternalStore } from 'react';

import ChatView from '~/components/ChatView';
import { BackgroundThreadContext } from '~/components/chat/backgroundThread';
import { Button } from '~/components/ui/button';
import {
  Dialog,
  DialogDescription,
  DialogHeader,
  DialogPopup,
  DialogTitle,
} from '~/components/ui/dialog';
import { ComposerHandleContext } from '~/composerHandleContext';
import { useQuietThreadShell } from '~/state/entities';
import { closeQuickView, openQuickView, quickViewStore } from './quick-view-store';
import './quick-view.css';

const ENVIRONMENT_ID = 'hoh-local';

export { openQuickView };

/** One modal host for every Home card; the conversation itself remains T3's ChatView. */
export function QuickViewDialog() {
  const { threadId } = useSyncExternalStore(quickViewStore.subscribe, quickViewStore.getSnapshot);
  const threadRef = threadId === null ? null : scopeThreadRef(ENVIRONMENT_ID, threadId);
  const thread = useQuietThreadShell(threadRef);
  const navigate = useNavigate();
  const title = thread?.title?.trim() || 'Conversation';
  const openFullConversation = useCallback(async () => {
    if (threadId === null) return;
    await navigate({
      to: '/$environmentId/$threadId',
      params: { environmentId: ENVIRONMENT_ID, threadId },
    });
    closeQuickView(false);
  }, [navigate, threadId]);
  // Embedded ChatView has no workspace chrome. If a timeline action needs it, continue in the full conversation.
  const parentPanel = useMemo(() => ({
    openToolDiff: () => { void openFullConversation(); },
    runShellCommand: () => { void openFullConversation(); },
  }), [openFullConversation]);

  return (
    <Dialog open={threadId !== null} onOpenChange={(open) => { if (!open) closeQuickView(); }}>
      <DialogPopup
        className="hoh-quick-view"
        bottomStickOnMobile={false}
        finalFocus={false}
      >
        {threadId === null ? null : (
          <>
            <DialogHeader className="hoh-quick-view__header">
              <span className="hoh-quick-view__heading">
                <DialogTitle className="hoh-quick-view__title">{title}</DialogTitle>
                <DialogDescription className="sr-only">
                  Quick view of this conversation. Reply here or open the full conversation.
                </DialogDescription>
              </span>
              <Button
                variant="ghost"
                size="sm"
                className="hoh-quick-view__full"
                aria-label={`Open ${title} in full conversation`}
                onClick={() => { void openFullConversation(); }}
              >
                <ArrowUpRightIcon aria-hidden />
                <span>Open full conversation</span>
              </Button>
            </DialogHeader>
            <div className="hoh-quick-view__chat" data-side-chat-pane={threadId}>
              <BackgroundThreadContext value>
                <ComposerHandleContext value={null}>
                  <ChatView
                    environmentId={ENVIRONMENT_ID}
                    threadId={threadId}
                    routeKind="server"
                    embedded={parentPanel}
                  />
                </ComposerHandleContext>
              </BackgroundThreadContext>
            </div>
          </>
        )}
      </DialogPopup>
    </Dialog>
  );
}
