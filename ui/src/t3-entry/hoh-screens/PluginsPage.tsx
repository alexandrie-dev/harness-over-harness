import { useEffect, useState } from 'react';
import type { PluginInventoryItem } from '../../hoh-host/plugin-inventory';
import './plugins.css';

type Inventory = { list(sessionId?: string): Promise<PluginInventoryItem[]> };
declare global { interface Window { __HOH_PLUGINS__?: Inventory } }

function selectedSession(): string | undefined {
  const match = window.location?.pathname.match(/^\/threads\/[^/]+\/([^/]+)\/?$/);
  if (!match) return undefined;
  try { return decodeURIComponent(match[1]); } catch { return undefined; }
}

export function PluginsPage({ inventory, session }: { inventory?: Inventory; session?: string } = {}) {
  const [plugins, setPlugins] = useState<PluginInventoryItem[] | null>(null);
  const [error, setError] = useState('');
  const sessionId = session ?? selectedSession();
  useEffect(() => {
    let stopped = false;
    const source = inventory ?? window.__HOH_PLUGINS__;
    const refresh = async () => {
      if (!source) { if (!stopped) setError('Plugin inventory is unavailable.'); return; }
      try { const result = await source.list(sessionId); if (!stopped) { setPlugins(result); setError(''); } }
      catch { if (!stopped) setError('Could not load plugin diagnostics.'); }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    return () => { stopped = true; window.clearInterval(timer); };
  }, [inventory, sessionId]);
  return (
    <main className="hoh-plugins">
      <header className="hoh-plugins__top"><div><span className="hoh-plugins__kicker">Extensions</span><h1>Plugins</h1><p>Compiled plugins and their diagnostics.</p></div></header>
      {error ? <p role="alert">{error}</p> : null}
      {!plugins && !error ? <p>Loading plugins…</p> : null}
      {plugins?.length === 0 ? <section className="hoh-plugins-empty" data-plugin-empty><h2>No plugins installed</h2></section> : null}
      {plugins?.length ? <div className="hoh-plugins__groups" data-plugin-list><section>
        <div className="hoh-plugin-group-title"><h2>Installed plugins</h2></div>
        <div className="hoh-plugin-grid">{plugins.map(plugin => (
          <article key={plugin.id} className="hoh-plugin-card" data-state={plugin.status === 'mounted' ? 'enabled' : 'blocked'} data-plugin-id={plugin.id}>
            <span className="hoh-plugin-eyebrow">Rust · {plugin.manifest.version ?? 'unknown version'}</span>
            <strong>{plugin.manifest.name ?? plugin.id}</strong>
            <span className="hoh-plugin-card__summary">{plugin.manifest.description ?? ''}</span>
            <span className="hoh-plugin-card__footer">{plugin.status}</span>
            {plugin.diagnostic ? <p>{plugin.diagnostic}</p> : null}
            {plugin.manifest.panel ? <section className="hoh-plugin-facts" aria-label={plugin.manifest.panel.title ?? 'Plugin panel'}>
              <h3>{plugin.manifest.panel.title ?? 'Plugin panel'}</h3>
              {!sessionId ? <p>Select a conversation to see its data.</p> : !plugin.data ? <p>{plugin.diagnostic ?? 'No finished turns yet.'}</p> : <dl>
                {(plugin.manifest.panel.fields ?? []).map(field => <div key={field.key}><dt>{field.label}</dt><dd>{String(plugin.data?.[field.key] ?? '—')}</dd></div>)}
              </dl>}
            </section> : null}
          </article>
        ))}</div>
      </section></div> : null}
    </main>
  );
}
