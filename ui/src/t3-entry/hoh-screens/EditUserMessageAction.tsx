import { useRef, useState } from 'react';
import { PencilIcon } from 'lucide-react';
import { useLocale } from 'gt-react';
import { ThreadId } from '@t3tools/contracts';
import {
  AlertDialog, AlertDialogClose, AlertDialogDescription, AlertDialogFooter,
  AlertDialogHeader, AlertDialogPopup, AlertDialogTitle,
} from '~/components/ui/alert-dialog';
import { Button } from '~/components/ui/button';
import { Textarea } from '~/components/ui/textarea';
import { Tooltip, TooltipPopup, TooltipTrigger } from '~/components/ui/tooltip';

type MessageEditApi = {
  canEdit: (threadId: string, messageId: string) => boolean;
  send: (threadId: string, messageId: string, text: string) => Promise<string>;
};

export function EditUserMessageAction({ threadId, messageId, text, disabled, onOpenChat }: {
  threadId: ThreadId;
  messageId: string;
  text: string;
  disabled: boolean;
  onOpenChat: (threadId: ThreadId) => void;
}) {
  const ru = useLocale().startsWith('ru');
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState('');
  const editButton = useRef<HTMLButtonElement>(null);
  const editor = useRef<HTMLTextAreaElement>(null);
  const sendingRef = useRef(false);
  const api = (window as Window & { __HOH_MESSAGE_EDIT__?: MessageEditApi }).__HOH_MESSAGE_EDIT__;
  if (!api || (!open && !api.canEdit(threadId, messageId))) return null;

  const label = ru ? 'Изменить сообщение' : 'Edit message';
  const canSend = !disabled && !sending && draft.trim() !== '' && draft.trim() !== text.trim();
  const send = async () => {
    if (!canSend || sendingRef.current) return;
    sendingRef.current = true;
    setSending(true);
    setError('');
    try {
      const branch = await api.send(threadId, messageId, draft);
      setOpen(false);
      onOpenChat(ThreadId.make(branch));
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
    } finally {
      sendingRef.current = false;
      setSending(false);
    }
  };

  return (
    <>
      <Tooltip>
        <TooltipTrigger render={
          <Button
            ref={editButton}
            type="button"
            size="xs"
            variant="ghost"
            aria-label={label}
            disabled={disabled}
            onClick={() => { setDraft(text); setError(''); setOpen(true); }}
          />
        }>
          <PencilIcon className="size-3" />
        </TooltipTrigger>
        <TooltipPopup side="top">{label}</TooltipPopup>
      </Tooltip>
      {open && (
        <AlertDialog open onOpenChange={next => { if (!next && !sendingRef.current) setOpen(false); }}>
          <AlertDialogPopup initialFocus={editor} finalFocus={editButton} className="overflow-y-auto">
            <form onSubmit={event => { event.preventDefault(); void send(); }}>
              <AlertDialogHeader>
                <AlertDialogTitle>{label}</AlertDialogTitle>
                <AlertDialogDescription>
                  {ru
                    ? 'Новая ветка продолжит переписку до этого сообщения и будет работать с текущими файлами проекта. Исходный чат сохранится.'
                    : 'The new branch uses the earlier messages and the current project files. The original chat stays available.'}
                </AlertDialogDescription>
                <Textarea
                  ref={editor}
                  aria-label={label}
                  dir="auto"
                  rows={3}
                  value={draft}
                  readOnly={sending}
                  onChange={event => setDraft(event.target.value)}
                />
                {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogClose render={<Button type="button" variant="outline" disabled={sending} />}>
                  {ru ? 'Отмена' : 'Cancel'}
                </AlertDialogClose>
                <Button type="submit" disabled={!canSend}>
                  {sending ? (ru ? 'Отправка…' : 'Sending…') : (ru ? 'Отправить в ветку' : 'Send in branch')}
                </Button>
              </AlertDialogFooter>
            </form>
          </AlertDialogPopup>
        </AlertDialog>
      )}
    </>
  );
}
