import { Tabs } from '@base-ui/react/tabs';
import { ArrowLeft, ArrowUpRight, AudioLines, ChevronRight, CircleAlert, Eye, GitBranch, PanelsTopLeft, Puzzle, Search, Wrench } from 'lucide-react';
import { useEffect, useRef, useState, type Ref } from 'react';
import type { PluginInventoryItem } from '../../hoh-host/plugin-inventory';
import { EntityLogo, entityText, pluginEntity } from './plugin-entities';
import { ExternalUiPlugins } from './ExternalUiPlugins';
import './plugins.css';

type Inventory = { list(sessionId?: string): Promise<PluginInventoryItem[]> };
declare global { interface Window { __HOH_PLUGINS__?: Inventory } }

const kinds = {
  panel: { label: 'Session panel', Icon: PanelsTopLeft },
  voice: { label: 'Voice', Icon: AudioLines },
  tool: { label: 'Tool', Icon: Wrench },
  observer: { label: 'Observer', Icon: Eye },
  extension: { label: 'Extension', Icon: Puzzle },
};

function pluginKind({ id, manifest }: PluginInventoryItem): keyof typeof kinds {
  if (manifest.panel) return 'panel';
  if (id === 'whisper-dictation') return 'voice';
  if (Array.isArray(manifest.permissions?.tools) && manifest.permissions.tools.length) return 'tool';
  if (Array.isArray(manifest.permissions?.observe) && manifest.permissions.observe.length) return 'observer';
  return 'extension';
}

function pluginStatus(status: string) {
  const label = status === 'mounted' ? 'Active' : status.replaceAll('-', ' ');
  return <span className="hoh-plugin-status" data-status={status}>{label}</span>;
}

function pluginIdentity(plugin: PluginInventoryItem) {
  const { name, description } = plugin.manifest;
  const displayName = typeof name === 'string' ? name : plugin.id;
  const kind = pluginKind(plugin);
  const { Icon } = kinds[kind];
  const entity = pluginEntity(displayName, plugin.id);
  return {
    kind,
    title: entityText(displayName, entity),
    description: typeof description === 'string' ? description : undefined,
    artwork: (
      <span className="hoh-plugin-artwork" aria-hidden="true">
        {entity ? <EntityLogo entity={entity} /> : <Icon />}
      </span>
    ),
  };
}

function PluginPermissions({ manifest }: { manifest: PluginInventoryItem['manifest'] }) {
  const permissions = [
    { label: 'Observed events', values: manifest.permissions?.observe },
    { label: 'Tools', values: manifest.permissions?.tools },
    { label: 'Journal records', values: manifest.permissions?.journal },
  ].flatMap(({ label, values }) => {
    const entries = Array.isArray(values) ? [...new Set(values.filter(value => typeof value === 'string'))] : [];
    return entries.length ? [{ label, entries }] : [];
  });

  return (
    <section className="hoh-plugin-section">
      <h2>Permissions</h2>
      {permissions.length ? (
        <dl className="hoh-plugin-data">
          {permissions.map(({ label, entries }) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd className="hoh-plugin-permissions">
                {entries.map(value => <code key={value}>{entityText(value)}</code>)}
              </dd>
            </div>
          ))}
        </dl>
      ) : <p>No permissions declared.</p>}
    </section>
  );
}

function PluginSessionData({ plugin, session }: { plugin: PluginInventoryItem; session?: string }) {
  const panel = plugin.manifest.panel;
  const fields = Array.isArray(panel?.fields)
    ? panel.fields.filter(field => typeof field?.key === 'string')
    : [];
  return (
    <section className="hoh-plugin-section">
      <h2>{entityText(panel?.title ?? 'Session data')}</h2>
      {!session ? <p>Select a conversation to see its data.</p> : !plugin.data ? (
        <p>{entityText(plugin.diagnostic ?? 'No finished turns yet.')}</p>
      ) : (
        <dl className="hoh-plugin-data">
          {fields.map(field => (
            <div key={field.key}>
              <dt>{entityText(field.label)}</dt>
              <dd>{entityText(String(plugin.data?.[field.key] ?? '—'))}</dd>
            </div>
          ))}
        </dl>
      )}
    </section>
  );
}

interface PluginDetailProps {
  plugin: PluginInventoryItem;
  session?: string;
  backRef: Ref<HTMLButtonElement>;
  onBack(): void;
}

function repositoryUrl(value: unknown): URL | null {
  if (typeof value !== 'string' || !/^https:\/\//i.test(value)) return null;
  try {
    const url = new URL(value);
    if (!url.hostname || url.hostname.startsWith('-') || url.username || url.password ||
        new TextEncoder().encode(url.href).byteLength > 2048) return null;
    return url;
  } catch {
    return null;
  }
}

function PluginDetail({ plugin, session, backRef, onBack }: PluginDetailProps) {
  const { kind, title, description, artwork } = pluginIdentity(plugin);
  const repository = repositoryUrl(plugin.manifest.repository);
  const github = repository?.hostname === 'github.com';
  return (
    <article className="hoh-plugin-detail" data-plugin-detail={plugin.id} data-kind={kind}>
      <header className="hoh-plugins__top">
        <button ref={backRef} className="hoh-plugin-back" type="button"
          onClick={onBack}>
          <ArrowLeft aria-hidden="true" /> All plugins
        </button>
        <div className="hoh-plugin-detail__hero">
          {artwork}
          <div className="hoh-plugin-detail__identity">
            <h1 className="hoh-plugins__title">{title}</h1>
            {description ? <p>{entityText(description)}</p> : null}
            <div className="hoh-plugin-detail__meta">
              {pluginStatus(plugin.status)}
              <span className="hoh-plugin-detail__kind">{kinds[kind].label}</span>
              {repository ? (
                <a className="hoh-plugin-source" href={repository.href} target="_blank" rel="noopener noreferrer">
                  {github ? <EntityLogo entity="github" /> : <GitBranch aria-hidden="true" />}
                  {github ? 'View on GitHub' : 'Source repository'}
                  <ArrowUpRight aria-hidden="true" />
                </a>
              ) : null}
            </div>
          </div>
        </div>
      </header>
      {plugin.diagnostic ? (
        <p className="hoh-plugin-diagnostic" role="status">
          <CircleAlert aria-hidden="true" /> <span>{entityText(plugin.diagnostic)}</span>
        </p>
      ) : null}
      <Tabs.Root defaultValue="overview" className="hoh-plugin-detail__tabs">
        <Tabs.List className="hoh-plugin-tabs" aria-label="Plugin details" activateOnFocus>
          <Tabs.Tab value="overview">Overview</Tabs.Tab>
          {plugin.manifest.panel ? <Tabs.Tab value="session">Session data</Tabs.Tab> : null}
        </Tabs.List>
        <div className="hoh-plugin-detail__body">
          <div className="hoh-plugin-detail__content">
            <Tabs.Panel value="overview"><PluginPermissions manifest={plugin.manifest} /></Tabs.Panel>
            {plugin.manifest.panel ? <Tabs.Panel value="session">
              <PluginSessionData plugin={plugin} session={session} />
            </Tabs.Panel> : null}
          </div>
          <aside className="hoh-plugin-info" aria-label="Plugin information">
            <h2>Plugin information</h2>
            <dl className="hoh-plugin-data">
              <div><dt>Identifier</dt><dd><code>{plugin.id}</code></dd></div>
              <div><dt>Version</dt><dd>{plugin.manifest.version ?? 'Unknown'}</dd></div>
              <div><dt>Language</dt><dd>{entityText('Rust')}</dd></div>
            </dl>
          </aside>
        </div>
      </Tabs.Root>
    </article>
  );
}

export function PluginsPage({ inventory, session }: { inventory?: Inventory; session?: string } = {}) {
  const [plugins, setPlugins] = useState<PluginInventoryItem[] | null>(null);
  const [error, setError] = useState('');
  const [query, setQuery] = useState('');
  const [selection, setSelection] = useState<{ id: string; open: boolean } | null>(null);
  const back = useRef<HTMLButtonElement>(null);
  const returningCard = useRef<HTMLButtonElement>(null);
  const search = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let stopped = false;
    let refreshing = false;
    const source = inventory ?? window.__HOH_PLUGINS__;
    if (!source) {
      setError('Plugin inventory is unavailable.');
      return;
    }

    const refresh = async () => {
      if (refreshing) return;
      refreshing = true;
      try {
        const result = await source.list(session);
        if (!stopped) {
          setPlugins(result);
          setError('');
        }
      } catch {
        if (!stopped) setError('Could not load plugin diagnostics.');
      } finally {
        refreshing = false;
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [inventory, session]);

  const selected = selection?.open ? plugins?.find(plugin => plugin.id === selection.id) : undefined;
  const detailOpen = !!selected;
  useEffect(() => {
    if (detailOpen) back.current?.focus();
    else if (selection) (returningCard.current ?? search.current)?.focus();
  }, [detailOpen, selection]);

  const filter = query.trim().toLowerCase();
  const visible = plugins?.filter(({ id, manifest }) => [id, manifest.name, manifest.description]
    .some(value => typeof value === 'string' && value.toLowerCase().includes(filter))) ?? [];

  return (
    <main className="hoh-plugins">
      {error ? <p className="hoh-plugin-diagnostic" role="alert">{error}</p> : null}
      {selected ? (
        <PluginDetail plugin={selected} session={session} backRef={back}
          onBack={() => setSelection({ id: selected.id, open: false })} />
      ) : (
        <>
          <header className="hoh-plugins__top hoh-plugins__catalog-header">
            <div>
              <h1 className="hoh-plugins__title">Plugins</h1>
              {plugins ? <p>{plugins.length} installed</p> : null}
            </div>
            {plugins?.length ? (
              <label className="hoh-plugins__search">
                <Search aria-hidden="true" />
                <input ref={search} type="search" aria-label="Search installed plugins"
                  placeholder="Search plugins" value={query} onChange={event => setQuery(event.target.value)} />
              </label>
            ) : null}
          </header>
          <ExternalUiPlugins />
          {!plugins && !error ? <p role="status">Loading plugins…</p> : null}
          {plugins ? (
            <section className="hoh-plugin-board" aria-label="Installed plugins">
              {visible.length ? (
                <div className="hoh-plugin-grid" data-plugin-list>
                  {visible.map(plugin => {
                    const { kind, title, description, artwork } = pluginIdentity(plugin);
                    const { label, Icon } = kinds[kind];
                    return (
                      <article key={plugin.id} className="hoh-plugin-tile" data-kind={kind}>
                        <span className="hoh-plugin-category"><Icon aria-hidden="true" />{label}</span>
                        <button type="button" className="hoh-plugin-card" data-plugin-id={plugin.id}
                          ref={plugin.id === selection?.id ? returningCard : undefined}
                          onClick={() => setSelection({ id: plugin.id, open: true })}>
                          <span className="hoh-plugin-card__header">
                            {artwork}
                            <strong>{title}</strong>
                            <ChevronRight className="hoh-plugin-card__arrow" aria-hidden="true" />
                          </span>
                          {description ? <span className="hoh-plugin-card__summary">
                            <span>{entityText(description)}</span>
                          </span> : null}
                          <span className="hoh-plugin-card__footer">
                            {plugin.manifest.version ? <span className="hoh-plugin-version">v{plugin.manifest.version}</span> : null}
                            <span className="hoh-plugin-card__language">{entityText('Rust')}</span>
                            {pluginStatus(plugin.status)}
                          </span>
                        </button>
                      </article>
                    );
                  })}
                </div>
              ) : (
                <div className="hoh-plugins-empty" data-plugin-empty={plugins.length === 0 ? '' : undefined}>
                  <Puzzle aria-hidden="true" />
                  <h2>{plugins.length ? 'No matching plugins' : 'No plugins installed'}</h2>
                  {plugins.length ? <button type="button" onClick={() => {
                    setQuery('');
                    search.current?.focus();
                  }}>Clear search</button> : null}
                </div>
              )}
            </section>
          ) : null}
        </>
      )}
    </main>
  );
}
