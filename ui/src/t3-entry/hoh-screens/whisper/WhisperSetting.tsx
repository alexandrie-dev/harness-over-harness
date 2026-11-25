import { useLocale } from 'gt-react';
import { useState, type FormEvent } from 'react';
import { Button } from '~/components/ui/button';
import { Input } from '~/components/ui/input';
import { saveWhisperServer, useWhisperServer, whisperFailureMessage } from '../../whisper/client';

export function WhisperSetting() {
  const serverUrl = useWhisperServer();
  const russian = useLocale() === 'ru';
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (serverUrl === undefined) return null;
  const value = draft ?? serverUrl ?? '';
  const title = russian ? 'Голосовой ввод Whisper' : 'Whisper dictation';

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSaving(true);
    setError(null);
    try {
      const result = await saveWhisperServer(value);
      if (result.kind === 'error') setError(whisperFailureMessage(result.failure, russian));
      if (result.kind === 'settings') setDraft(null);
    } catch {
      setError(russian ? 'Не удалось сохранить адрес Whisper.' : 'Could not save the Whisper address.');
    } finally {
      setSaving(false);
    }
  }

  return (
    <section id="hoh-whisper-settings" className="space-y-3 rounded-xl border border-border/70 bg-card/40 p-4"
      aria-labelledby="hoh-whisper-title">
      <h2 id="hoh-whisper-title" className="text-sm font-medium">{title}</h2>
      <form className="space-y-2" onSubmit={save}>
        <label htmlFor="hoh-whisper-url" className="text-sm">
          {russian ? 'Адрес whisper-daemon' : 'whisper-daemon address'}
        </label>
        <div className="flex items-center gap-2">
          <Input id="hoh-whisper-url" type="url" placeholder="http://127.0.0.1:8765" value={value}
            maxLength={2048} spellCheck={false} autoComplete="off" disabled={saving}
            aria-invalid={error !== null} aria-describedby={error ? 'hoh-whisper-error' : 'hoh-whisper-description'}
            onChange={event => { setDraft(event.target.value); setError(null); }} />
          <Button type="submit" variant="outline" disabled={saving || value.trim() === (serverUrl ?? '')}>
            {russian ? 'Сохранить' : 'Save'}
          </Button>
        </div>
        <p id="hoh-whisper-description" className="text-xs text-muted-foreground">
          {russian
            ? 'Записи отправляются по этому адресу при завершении диктовки. Пустой адрес отключает запись.'
            : 'Recordings go to this address when you finish dictating. Clear the address to disable recording.'}
        </p>
        {error && <p id="hoh-whisper-error" role="alert" className="text-sm text-destructive">{error}</p>}
      </form>
    </section>
  );
}
