import { invoke } from '@tauri-apps/api/core';
import { RotateCcwIcon } from 'lucide-react';
import { ProviderInstanceIcon } from '~/components/chat/ProviderInstanceIcon';
import { PROVIDER_PRESENTATION } from '~/components/usage/usageProviders';
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
    <section className="hoh-usage__provider" aria-label={title} data-provider={provider.executor}>
      <header className="hoh-usage__provider-header">
        <h2><ProviderInstanceIcon driverKind={PROVIDER_PRESENTATION[provider.executor].driverKind} displayName={title} iconClassName="size-4" />{title}</h2>
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
        const windowMs = (window.windowMinutes ?? 0) * 60_000;
        const pace = reset && windowMs > 0
          ? Math.max(0, Math.min(100, 100 * (reset.getTime() - Date.now()) / windowMs))
          : null;
        const hours = reset ? Math.max(0, Math.ceil((reset.getTime() - Date.now()) / 3_600_000)) : 0;
        const countdown = hours >= 24 ? `${Math.floor(hours / 24)}d ${hours % 24}h` : hours ? `${hours}h` : '<1h';
        return (
          <div className="hoh-usage__window" key={`${label}:${index}`}>
            <div className="hoh-usage__window-title">
              <span>{label}</span>
              <span><strong>{window.remainingPercent}%</strong> left</span>
            </div>
            <div className="hoh-usage__track">
              <progress aria-label={`${title} ${label} remaining`} value={window.remainingPercent} max={100} />
              {pace !== null ? (
                <span
                  className="hoh-usage__pace"
                  style={{ insetInlineStart: `clamp(2px, ${pace}%, calc(100% - 2px))` }}
                  title={`${Math.round(pace)}% of the limit window remains`}
                />
              ) : null}
              {reset ? (
                <span className="hoh-usage__reset" title={reset.toLocaleString()}>
                  <RotateCcwIcon aria-hidden />
                  <time aria-label={`Resets ${dateTime.format(reset)}`} dateTime={reset.toISOString()}>{countdown}</time>
                </span>
              ) : null}
            </div>
          </div>
        );
      })}
    </section>
  );
}

export function UsageLimitsSection({ now, hiddenProviders }: { now: number; hiddenProviders: ReadonlySet<string> }) {
  const [states, setStates] = useState<Record<Provider['executor'], ProviderState>>({
    codex: { status: 'loading' },
    claude: { status: 'loading' },
  });

  useEffect(() => {
    let current = true;
    setStates((previous) => ({
      codex: { ...previous.codex, status: 'loading' },
      claude: { ...previous.claude, status: 'loading' },
    }));
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
  }, [now]);

  const visible = providers.filter(({ executor }) => !hiddenProviders.has(executor));
  if (visible.length === 0) return <p className="text-sm text-muted-foreground">Select a provider to see limits.</p>;
  return (
    <div className="hoh-usage">
      {visible.map((provider) => <ProviderLimits key={provider.executor} provider={provider} state={states[provider.executor]} />)}
    </div>
  );
}
