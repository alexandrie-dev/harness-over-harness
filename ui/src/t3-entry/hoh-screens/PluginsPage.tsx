import {
  ArrowLeftIcon, CheckIcon, ChevronDownIcon, CopyIcon,
  LockKeyholeIcon, MoreHorizontalIcon, PackageOpenIcon, PuzzleIcon, Trash2Icon,
} from 'lucide-react';
import { useState } from 'react';

import { Badge } from '~/components/ui/badge';
import { Button } from '~/components/ui/button';
import {
  Dialog, DialogAction, DialogClose, DialogDescription, DialogFooter, DialogHeader,
  DialogPopup, DialogTitle,
} from '~/components/ui/dialog';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '~/components/ui/menu';
import { writeTextToClipboard } from '~/hooks/useCopyToClipboard';
import './plugins.css';

type PluginLanguage = 'MD' | 'TS' | 'Rust' | 'Go';
type PluginState = 'enabled' | 'quarantined' | 'update-pending' | 'blocked' | 'disabled' | 'approval-pending';

interface Plugin {
  id: string;
  name: string;
  publisher: string;
  version: string;
  previousVersion?: string;
  languages: readonly PluginLanguage[];
  state: PluginState;
  builtIn: boolean;
  summary: string;
  contributions: readonly string[];
  replacements: readonly string[];
  permissions: readonly string[];
  fingerprint: string;
}

const PLUGIN_STATE_LABELS: Readonly<Record<PluginState, string>> = {
  enabled: 'Enabled', quarantined: 'Quarantined · approval needed',
  'update-pending': 'Update approval needed', blocked: 'Blocked', disabled: 'Disabled',
  'approval-pending': 'Approval pending',
};

// The engine does not expose plugin inventory yet. Keep this honest and empty until it does.
export const INSTALLED_PLUGINS: readonly Plugin[] = [];

function pluginTone(id: string): number {
  let hash = 0;
  for (const character of id) hash = (hash * 31 + character.charCodeAt(0)) >>> 0;
  return hash % 6;
}

function FolderTile({ plugin }: { plugin: Plugin }) {
  return (
    <span className="hoh-plugin-folder" data-tone={pluginTone(plugin.id)} aria-hidden="true">
      <span className="hoh-plugin-folder__back" />
      <span className="hoh-plugin-folder__front"><span /></span>
    </span>
  );
}

function StatusBadge({ state }: { state: PluginState }) {
  const variant = state === 'enabled' ? 'info' : state === 'disabled' ? 'secondary' : state === 'blocked' ? 'error' : 'warning';
  return (
    <Badge variant={variant} className="hoh-plugin-status" data-plugin-status={state}>
      {state === 'blocked' ? <LockKeyholeIcon /> : null}
      {PLUGIN_STATE_LABELS[state]}
    </Badge>
  );
}

function LanguageBadges({ plugin }: { plugin: Plugin }) {
  return <span className="hoh-plugin-languages">{plugin.languages.map((language) => <Badge key={language} variant="outline">{language}</Badge>)}</span>;
}

function PluginIdentity({ plugin }: { plugin: Plugin }) {
  return (
    <div className="hoh-plugin-identity">
      <FolderTile plugin={plugin} />
      <div className="hoh-plugin-identity__copy">
        <span className="hoh-plugin-eyebrow">{plugin.publisher} · {plugin.version}</span>
        <strong>{plugin.name}</strong>
      </div>
    </div>
  );
}

function FactList({ title, values, empty = 'None' }: { title: string; values: readonly string[]; empty?: string }) {
  return (
    <section className="hoh-plugin-facts">
      <h3>{title}</h3>
      {values.length ? <ul>{values.map((value) => <li key={value}>{value}</li>)}</ul> : <p>{empty}</p>}
    </section>
  );
}

interface DetailProps {
  plugin: Plugin;
  copied: string | null;
  copyError: string | null;
  onBack(): void;
  onCopy(label: string, value: string): void;
  onRequestState(id: string): void;
  onRequestRemove(plugin: Plugin): void;
}

/** The retained variant-C detail presentation, opened from the variant-A card list. */
function PluginDetail({ plugin, copied, copyError, onBack, onCopy, onRequestState, onRequestRemove }: DetailProps) {
  const shortFingerprint = `${plugin.fingerprint.slice(0, 19)}…${plugin.fingerprint.slice(-8)}`;
  const action = plugin.state === 'enabled' ? 'Disable' : 'Enable';
  const blocked = plugin.state === 'blocked';
  return (
    <article className="hoh-plugin-detail" data-plugin-detail={plugin.id}>
      <header className="hoh-plugin-detail__header">
        <Button variant="ghost" size="sm" onClick={onBack}><ArrowLeftIcon />All plugins</Button>
        <div className="hoh-plugin-detail__hero">
          <PluginIdentity plugin={plugin} />
          <div className="hoh-plugin-detail__state"><StatusBadge state={plugin.state} /><LanguageBadges plugin={plugin} /></div>
        </div>
        <p>{plugin.summary}</p>
        <div className="hoh-plugin-detail__actions">
          <Button size="sm" disabled={blocked} onClick={() => onRequestState(plugin.id)}>
            {blocked ? <LockKeyholeIcon /> : <PuzzleIcon />}{blocked ? 'Blocked by policy' : action}
          </Button>
          <Button size="sm" variant="destructive-outline" onClick={() => onRequestRemove(plugin)}><Trash2Icon />Remove</Button>
          <DropdownMenu>
            <DropdownMenuTrigger render={<Button size="sm" variant="outline" />} aria-label="More plugin actions">
              <MoreHorizontalIcon />More<ChevronDownIcon />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onClick={() => onCopy('Plugin ID', plugin.id)}><CopyIcon />Copy plugin ID</DropdownMenuItem>
              <DropdownMenuItem onClick={() => onCopy('Fingerprint', plugin.fingerprint)}><CopyIcon />Copy full fingerprint</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          {copied ? <span className="hoh-plugin-copied" role="status"><CheckIcon />{copied} copied</span> : null}
          {copyError ? <span className="hoh-plugin-copy-error" role="status">{copyError}</span> : null}
        </div>
      </header>
      <div className="hoh-plugin-detail__grid">
        <FactList title="Adds" values={plugin.contributions} />
        <FactList title="Replaces" values={plugin.replacements} />
        <FactList title="Permissions and data" values={plugin.permissions} />
        <section className="hoh-plugin-facts hoh-plugin-versions">
          <h3>Versions and fingerprint</h3>
          <dl>
            <div><dt>Installed</dt><dd>{plugin.version}</dd></div>
            {plugin.previousVersion ? <div><dt>Previous</dt><dd>{plugin.previousVersion}</dd></div> : null}
            <div><dt>SHA-256</dt><dd><code>{shortFingerprint}</code><Button aria-label="Copy SHA-256 fingerprint" size="icon-micro" variant="ghost" onClick={() => onCopy('Fingerprint', plugin.fingerprint)}><CopyIcon /></Button></dd></div>
          </dl>
        </section>
      </div>
    </article>
  );
}

function CardGrid({ plugins, onOpen }: { plugins: readonly Plugin[]; onOpen(plugin: Plugin): void }) {
  return (
    <div className="hoh-plugin-grid">
      {plugins.map((plugin) => (
        <button key={plugin.id} type="button" className="hoh-plugin-card" data-state={plugin.state} data-plugin-id={plugin.id} onClick={() => onOpen(plugin)}>
          <FolderTile plugin={plugin} />
          <span className="hoh-plugin-eyebrow">{plugin.publisher} · {plugin.version}</span>
          <strong>{plugin.name}</strong>
          <span className="hoh-plugin-card__summary">{plugin.summary}</span>
          <span className="hoh-plugin-card__footer"><StatusBadge state={plugin.state} /><LanguageBadges plugin={plugin} /></span>
        </button>
      ))}
    </div>
  );
}

function EmptyPlugins() {
  return (
    <section className="hoh-plugins-empty" data-plugin-empty>
      <span className="hoh-plugins-empty__icon"><PackageOpenIcon aria-hidden /></span>
      <h2>No plugins installed</h2>
      <p>Plugins can be added from a local folder once plugin installation is available in HoH.</p>
    </section>
  );
}

export function PluginsPage() {
  const [statuses, setStatuses] = useState<Partial<Record<string, PluginState>>>({});
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [removePlugin, setRemovePlugin] = useState<Plugin | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [copyError, setCopyError] = useState<string | null>(null);
  const plugins = INSTALLED_PLUGINS.map((plugin) => ({ ...plugin, state: statuses[plugin.id] ?? plugin.state }));
  const selected = plugins.find((plugin) => plugin.id === selectedId) ?? null;

  const openPlugin = (plugin: Plugin) => {
    document.querySelector('.hoh-page')?.scrollTo({ top: 0 });
    setSelectedId(plugin.id); setCopied(null); setCopyError(null);
  };
  const copy = (label: string, value: string) => {
    setCopied(null); setCopyError(null);
    void writeTextToClipboard(value, {
      target: label,
      unavailableMessage: `Clipboard is unavailable while copying ${label.toLowerCase()}.`,
      failureMessage: `Failed to copy ${label.toLowerCase()}.`,
    }).then((didCopy) => { if (didCopy) setCopied(label); }, (error: Error) => setCopyError(error.message));
  };
  const requestState = (id: string) => setStatuses((current) => ({ ...current, [id]: 'approval-pending' }));
  const detailProps = { copied, copyError, onCopy: copy, onRequestState: requestState, onRequestRemove: setRemovePlugin };

  return (
    <main className="hoh-plugins">
      <header className="hoh-plugins__top">
        <div><span className="hoh-plugins__kicker">Extensions</span><h1>Plugins</h1><p>Tools, skills, pages, and controls that extend HoH.</p></div>
        <span className="hoh-plugins__coming-soon">Coming Soon</span>
      </header>

      {selected ? <PluginDetail plugin={selected} onBack={() => setSelectedId(null)} {...detailProps} /> : plugins.length > 0 ? (
        <div className="hoh-plugins__groups" data-plugin-list>
          <section>
            <div className="hoh-plugin-group-title"><h2>Installed plugins</h2><span>Added to this installation</span></div>
            <CardGrid plugins={plugins} onOpen={openPlugin} />
          </section>
        </div>
      ) : <EmptyPlugins />}

      <Dialog open={removePlugin !== null} onOpenChange={(open) => { if (!open) setRemovePlugin(null); }}>
        <DialogPopup>
          <DialogHeader><DialogTitle>Remove {removePlugin?.name}?</DialogTitle><DialogDescription>HoH will ask for confirmation before changing installed plugins.</DialogDescription></DialogHeader>
          <DialogFooter>
            <DialogClose render={<Button variant="outline" />}>Cancel</DialogClose>
            <DialogAction onClick={() => { if (removePlugin) requestState(removePlugin.id); setRemovePlugin(null); }}>Request removal</DialogAction>
          </DialogFooter>
        </DialogPopup>
      </Dialog>
    </main>
  );
}
