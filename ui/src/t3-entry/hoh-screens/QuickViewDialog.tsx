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
import { closeQuickView, openFullFromQuickView, openQuickView, quickViewStore } from './quick-view-store';
import './quick-view.css';

const ENVIRONMENT_ID = 'hoh-local';
const FRAME_VARIANT_KEY = 'hoh.quick-view-frame';
const EMPTY_SUBSCRIBE = () => () => {};
const EMPTY_VERSION = () => 0;

type QuickViewFrameState = 'working' | 'waiting' | 'ready';
type HomeState = 'working' | 'waiting' | 'done' | 'ended';
interface QuickViewHome {
  version(): number;
  subscribe(listener: () => void): () => void;
  entries(): ReadonlyArray<{ readonly id: string; readonly state: HomeState }>;
}

function quickViewHome(): QuickViewHome | undefined {
  return (globalThis as { __HOH_HOME__?: QuickViewHome }).__HOH_HOME__;
}

/** Temporary design-review switch: `localStorage[hoh.quick-view-frame] = "2"`; variant 1 is canonical. */
export function quickViewFrameVariant(): '1' | '2' {
  try {
    return globalThis.localStorage?.getItem(FRAME_VARIANT_KEY) === '2' ? '2' : '1';
  } catch {
    return '1';
  }
}

export function quickViewFrameState(
  homeState: HomeState | undefined,
  thread: {
    readonly hasPendingApprovals: boolean;
    readonly hasPendingUserInput: boolean;
    readonly pendingSubagentRequest?: 'approval' | 'user_input' | null;
    readonly runtime: { readonly status: string } | null;
  } | null,
): QuickViewFrameState {
  if (homeState === 'waiting') return 'waiting';
  if (homeState === 'working') return 'working';
  if (
    thread?.hasPendingApprovals ||
    thread?.hasPendingUserInput ||
    thread?.pendingSubagentRequest === 'approval' ||
    thread?.pendingSubagentRequest === 'user_input'
  ) return 'waiting';
  return thread?.runtime && ['preparing', 'queued', 'starting', 'running'].includes(thread.runtime.status)
    ? 'working'
    : 'ready';
}

export { openQuickView };

/** One modal host for every Home card; the conversation itself remains T3's ChatView. */
export function QuickViewDialog() {
  const { threadId } = useSyncExternalStore(quickViewStore.subscribe, quickViewStore.getSnapshot);
  const threadRef = threadId === null ? null : scopeThreadRef(ENVIRONMENT_ID, threadId);
  const thread = useQuietThreadShell(threadRef);
  const home = quickViewHome();
  useSyncExternalStore(home?.subscribe ?? EMPTY_SUBSCRIBE, home?.version ?? EMPTY_VERSION);
  const homeState = home?.entries().find((entry) => entry.id === threadId)?.state;
  const frameState = quickViewFrameState(homeState, thread);
  const frameVariant = quickViewFrameVariant();
  const navigate = useNavigate();
  const title = thread?.title?.trim() || 'Conversation';
  const openFullConversation = useCallback(() => {
    openFullFromQuickView((threadId) => { void navigate({
      to: '/$environmentId/$threadId',
      params: { environmentId: ENVIRONMENT_ID, threadId },
    }); });
  }, [navigate]);
  // Embedded ChatView has no workspace chrome. If a timeline action needs it, continue in the full conversation.
  const parentPanel = useMemo(() => ({
    openToolDiff: () => { void openFullConversation(); },
    runShellCommand: () => { void openFullConversation(); },
  }), [openFullConversation]);

  return (
    <Dialog open={threadId !== null} onOpenChange={(open) => { if (!open) closeQuickView(); }}>
      <DialogPopup
        className="hoh-quick-view"
        data-frame-state={frameState}
        data-frame-variant={frameVariant}
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
