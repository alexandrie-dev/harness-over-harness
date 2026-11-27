import { invoke, isTauri } from '@tauri-apps/api/core';
import { useLocale } from 'gt-react';
import { LockKeyholeIcon } from 'lucide-react';
import { useId, useRef, useState, type FormEvent } from 'react';
import { ComposerControl, ComposerControlIcon } from '~/components/chat/ComposerControl';
import { Button } from '~/components/ui/button';
import { Dialog, DialogDescription, DialogFooter, DialogHeader, DialogPopup, DialogTitle, DialogTrigger } from '~/components/ui/dialog';
import { Input } from '~/components/ui/input';
import { Tooltip, TooltipPopup, TooltipTrigger } from '~/components/ui/tooltip';

export function ChatSecretAction({ onInsert }: { onInsert: (path: string) => boolean }) {
  const russian = useLocale().startsWith('ru');
  const title = russian ? 'Вставить секрет' : 'Insert secret';
  const inputId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const multiline = useRef('');
  const inFlight = useRef(false);
  const [open, setOpen] = useState(false);
  const [hasMultiline, setHasMultiline] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  function clear() {
    multiline.current = '';
    setHasMultiline(false);
    if (inputRef.current) inputRef.current.value = '';
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    event.stopPropagation();
    if (inFlight.current) return;
    const secret = multiline.current || inputRef.current?.value || '';
    if (!secret.trim()) {
      setError(russian ? 'Вставьте секрет.' : 'Enter a secret.');
      return;
    }
    if (new TextEncoder().encode(secret).length > 64 * 1024) {
      setError(russian ? 'Размер секрета превышает 64 КиБ.' : 'The secret exceeds 64 KiB.');
      return;
    }
    inFlight.current = true;
    setBusy(true);
    setError('');
    try {
      const path = await invoke<string>('store_chat_secret', { secret });
      if (onInsert(path)) {
        clear();
        setOpen(false);
      } else {
        setError(russian ? 'Не удалось добавить путь в черновик.' : 'Could not add the path to the draft.');
      }
    } catch {
      setError(russian ? 'Не удалось сохранить секрет.' : 'Could not save the secret.');
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  }

  if (!isTauri()) return null;
  return (
    <Dialog open={open} onOpenChange={(next) => {
      if (inFlight.current) return;
      if (!next) clear();
      setError('');
      setOpen(next);
    }}>
      <Tooltip>
        <TooltipTrigger render={<DialogTrigger render={<ComposerControl type="button" size="sm" aria-label={title} />} />}>
          <ComposerControlIcon icon={LockKeyholeIcon} size="sm" />
        </TooltipTrigger>
        <TooltipPopup side="top">{title}</TooltipPopup>
      </Tooltip>
      <DialogPopup initialFocus={inputRef} showCloseButton={false}>
        <form onSubmit={submit} autoComplete="off" data-v2-item-type="secret_request">
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
            <DialogDescription>
              {russian
                ? 'В черновик попадёт только путь. Команды агента смогут прочитать секрет из закрытого файла до выхода из HoH, максимум сутки.'
                : 'Only the path goes into the draft. The agent’s commands can read the secret from a private file until you quit HoH, for up to 24 hours.'}
            </DialogDescription>
            <label htmlFor={inputId} className="mt-2 text-sm font-medium">
              {russian ? 'Токен, пароль или ключ' : 'Token, password or key'}
            </label>
            <Input
              ref={inputRef}
              id={inputId}
              type="password"
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
              data-1p-ignore
              data-lpignore="true"
              data-bwignore
              readOnly={busy}
              placeholder={hasMultiline ? (russian ? 'Многострочный секрет вставлен' : 'Multiline secret pasted') : undefined}
              onChange={() => {
                multiline.current = '';
                setHasMultiline(false);
                setError('');
              }}
              onPaste={(event) => {
                const value = event.clipboardData.getData('text/plain');
                if (!/[\r\n]/.test(value)) return;
                event.preventDefault();
                if (inFlight.current) return;
                multiline.current = value;
                event.currentTarget.value = '';
                setHasMultiline(true);
                setError('');
              }}
            />
            {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="outline" disabled={busy} onClick={() => {
              clear();
              setOpen(false);
            }}>
              {russian ? 'Отмена' : 'Cancel'}
            </Button>
            <Button type="submit" disabled={busy}>
              {russian ? 'Добавить в черновик' : 'Add to draft'}
            </Button>
          </DialogFooter>
        </form>
      </DialogPopup>
    </Dialog>
  );
}
