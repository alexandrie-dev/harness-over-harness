import { invoke } from '@tauri-apps/api/core';
import { RefreshCwIcon, XIcon } from 'lucide-react';
import { useEffect, useState } from 'react';

import type { ExecutorLimitsResult } from '../../protocol';
import './usage.css';

const providers = [
  { executor: 'codex', title: 'Codex' },
  { executor: 'claude', title: 'Claude' },
] as const;

type Provider = typeof providers[number];
type LimitWindow = ExecutorLimitsResult['windows'][number];
type Snapshot = { windows: LimitWindow[]; checkedAt: number };
type ProviderState = { status: ExecutorLimitsResult['state'] | 'loading'; snapshot?: Snapshot };
const dateTime = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });

function windowLabel(window: LimitWindow) {
  const minutes = window.windowMinutes;
  let duration = 'Account limit';
  if (minutes === 10080) duration = 'Weekly';
  else if (minutes === 1440) duration = 'Daily';
  else if (minutes) duration = minutes % 60 === 0 ? `${minutes / 60} hr` : `${minutes} min`;
  return window.name ? `${duration} · ${window.name}` : duration;
}

const inFlight = new Map<Provider['executor'], Promise<ExecutorLimitsResult>>();

function readLimits(executor: Provider['executor']) {
  const pending = inFlight.get(executor);
  if (pending) return pending;
  const request = invoke<ExecutorLimitsResult>('engine_request', {
    method: 'hoh/executor/limits',
    params: { executor },
  }).catch((): ExecutorLimitsResult => ({ state: 'failed', windows: [] }))
    .finally(() => { inFlight.delete(executor); });
  inFlight.set(executor, request);
  return request;
}

function ProviderLimits({ provider, state }: { provider: Provider; state: ProviderState }) {
  const { title } = provider;
  const { status, snapshot } = state;

  const message = {
    loading: 'Checking account limits…',
    available: snapshot?.windows.length ? '' : 'The provider returned no quota windows for this account.',
    unavailable: 'This provider does not expose account quotas through its HoH integration.',
    failed: 'Could not read account limits. Check that the provider is signed in, then refresh.',
  }[status];

  return (
    <section className="hoh-usage__provider" aria-label={title}>
      <header className="hoh-usage__provider-header">
        <h2>{title}</h2>
        {snapshot ? (
          <time dateTime={new Date(snapshot.checkedAt).toISOString()}>
            {status === 'available' ? 'Updated' : 'Last successful check'} {dateTime.format(snapshot.checkedAt)}
          </time>
        ) : null}
      </header>
      {message ? <p role="status">{message}</p> : null}
      {snapshot?.windows.map((window, index) => {
        const label = windowLabel(window);
        const reset = window.resetsAt ? new Date(window.resetsAt * 1000) : undefined;
        return (
          <div className="hoh-usage__window" key={`${label}:${index}`}>
            <div className="hoh-usage__window-title">
              <span>{label}</span>
              <span><strong>{window.remainingPercent}%</strong> left</span>
            </div>
            <progress aria-label={`${title} ${label} remaining`} value={window.remainingPercent} max={100} />
            {reset ? (
              <span className="hoh-usage__reset" title={reset.toLocaleString()}>
                Resets <time dateTime={reset.toISOString()}>{dateTime.format(reset)}</time>
              </span>
            ) : null}
          </div>
        );
      })}
    </section>
  );
}

export function UsagePage({ onClose }: { onClose: () => void }) {
  const [revision, setRevision] = useState(0);
  const [states, setStates] = useState<Record<Provider['executor'], ProviderState>>({
    codex: { status: 'loading' },
    claude: { status: 'loading' },
  });
  const refreshing = providers.some(({ executor }) => states[executor].status === 'loading');

  useEffect(() => {
    let current = true;
    for (const { executor } of providers) {
      void readLimits(executor).then((result) => {
        if (!current) return;
        const snapshot = { windows: result.windows, checkedAt: Date.now() };
        setStates((previous) => ({
          ...previous,
          [executor]: {
            status: result.state,
            snapshot: result.state === 'available' ? snapshot : previous[executor].snapshot,
          },
        }));
      });
    }
    return () => { current = false; };
  }, [revision]);

  function refresh() {
    setStates((previous) => ({
      codex: { ...previous.codex, status: 'loading' },
      claude: { ...previous.claude, status: 'loading' },
    }));
    setRevision((previous) => previous + 1);
  }

  return (
    <main className="hoh-usage" aria-labelledby="hoh-usage-heading">
      <header className="hoh-usage__header">
        <div>
          <h1 id="hoh-usage-heading">Usage</h1>
          <p>Account limits</p>
        </div>
        <div className="hoh-usage__actions">
          <button type="button" onClick={refresh} disabled={refreshing}>
            <RefreshCwIcon aria-hidden /> Refresh
          </button>
          <button type="button" aria-label="Close usage" onClick={onClose}>
            <XIcon aria-hidden />
          </button>
        </div>
      </header>
      {providers.map((provider) => <ProviderLimits key={provider.executor} provider={provider} state={states[provider.executor]} />)}
    </main>
  );
}
