import { useState } from 'react';

interface Props {
  threadId: string;
  path: string;
  label: string;
  onApproved: () => Promise<unknown>;
}

export function ExternalImagePrompt({ threadId, path, label, onApproved }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  async function show() {
    const host = window as Window & {
      __HOH_EXTERNAL_IMAGE__?: (threadId: string, path: string) => Promise<boolean>;
    };
    setBusy(true);
    setError(undefined);
    try {
      if (await host.__HOH_EXTERNAL_IMAGE__?.(threadId, path)) await onApproved();
    } catch (failure) {
      setError(typeof failure === 'string' ? failure : 'The image could not be opened.');
    } finally {
      setBusy(false);
    }
  }

  return (
    <span className="my-2 flex w-fit max-w-full flex-wrap items-center gap-2 rounded-md border border-border bg-card px-3 py-2 text-sm">
      <span className="min-w-0 break-all">{label}</span>
      <button type="button" className="rounded border border-border px-2 py-1 hover:bg-accent"
        disabled={busy} onClick={() => void show()}>
        {busy ? 'Waiting for confirmation…' : 'Show external image'}
      </button>
      {error && <span role="alert" className="w-full text-destructive">{error}</span>}
    </span>
  );
}
