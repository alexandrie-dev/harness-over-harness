import { useLocale } from 'gt-react';
import { LoaderCircleIcon, SparklesIcon } from 'lucide-react';
import { useId, useState, useSyncExternalStore } from 'react';

import { Button } from '~/components/ui/button';
import { Switch } from '~/components/ui/switch';
import { toastManager } from '~/components/ui/toast';

interface ChatTitles {
  getEnabled(): boolean;
  setEnabled(enabled: boolean): Promise<void>;
  subscribe(listener: () => void): () => void;
  rename(threadId: string): Promise<string>;
}

function chatTitles(): ChatTitles | undefined {
  return (globalThis as { __HOH_CHAT_TITLES__?: ChatTitles }).__HOH_CHAT_TITLES__;
}

const subscribe = (listener: () => void) => chatTitles()?.subscribe(listener) ?? (() => {});
const getEnabled = () => chatTitles()?.getEnabled() ?? true;

export function ChatTitleSetting() {
  const enabled = useSyncExternalStore(subscribe, getEnabled);
  const russian = useLocale() === 'ru';
  const labelId = useId();
  const descriptionId = useId();
  const [saving, setSaving] = useState(false);
  const titles = chatTitles();
  if (!titles) return null;

  const setEnabled = async (checked: boolean) => {
    setSaving(true);
    try {
      await titles.setEnabled(checked);
    } catch {
      toastManager.add({
        type: 'error',
        title: russian ? 'Не удалось подтвердить настройку автоназваний' : 'Could not confirm the automatic title setting',
      });
    } finally {
      setSaving(false);
    }
  };

  return (
    <section id="hoh-chat-title-settings" className="rounded-xl border border-border/70 bg-card/40 px-3 py-3 sm:px-4" aria-labelledby={labelId}>
      <div className="flex items-center justify-between gap-4">
        <div className="min-w-0">
          <h2 id={labelId} className="text-sm font-medium text-foreground">
            {russian ? 'Автоназвания чатов' : 'Automatic chat titles'}
          </h2>
          <p id={descriptionId} className="mt-1 text-xs leading-5 text-muted-foreground">
            {russian ? 'Давать новым чатам короткие названия по первому сообщению.' : 'Give new chats short titles based on their first message.'}
          </p>
        </div>
        <Switch aria-labelledby={labelId} aria-describedby={descriptionId} checked={enabled} disabled={saving} onCheckedChange={setEnabled} />
      </div>
    </section>
  );
}

export function RenameChatTitleButton({ threadId, title, className }: { threadId: string; title: string; className?: string }) {
  const russian = useLocale() === 'ru';
  const [pending, setPending] = useState(false);
  const titles = chatTitles();
  if (!titles) return null;

  const label = russian ? 'Придумать название чата' : 'Generate chat title';
  const pendingLabel = russian ? 'Подбираем название чата' : 'Generating chat title';

  const rename = async () => {
    if (pending) return;
    setPending(true);
    try {
      await titles.rename(threadId);
    } catch (error) {
      toastManager.add({
        type: 'error',
        title: russian ? 'Не удалось подобрать название чата' : 'Could not generate a chat title',
        description: error instanceof Error ? error.message : undefined,
      });
    } finally {
      setPending(false);
    }
  };

  return (
    <Button
      type="button"
      className={className}
      variant="ghost-muted"
      size="icon-xs"
      title={pending ? pendingLabel : label}
      aria-label={`${pending ? pendingLabel : label} · ${title}`}
      aria-busy={pending || undefined}
      aria-disabled={pending || undefined}
      onClick={() => { void rename(); }}
    >
      {pending ? <LoaderCircleIcon aria-hidden className="motion-safe:animate-spin" /> : <SparklesIcon aria-hidden />}
    </Button>
  );
}
