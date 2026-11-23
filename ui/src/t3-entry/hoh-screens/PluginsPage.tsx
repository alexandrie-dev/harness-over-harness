import {
  ArrowLeftIcon, CheckIcon, ChevronDownIcon, ChevronRightIcon, CopyIcon, FolderPlusIcon,
  LockKeyholeIcon, MoreHorizontalIcon, PackageOpenIcon, PuzzleIcon, Trash2Icon,
} from 'lucide-react';
import { useState } from 'react';

import { Badge } from '~/components/ui/badge';
import { Button } from '~/components/ui/button';
import {
  Dialog, DialogAction, DialogClose, DialogDescription, DialogFooter, DialogHeader,
  DialogPanel, DialogPopup, DialogTitle,
} from '~/components/ui/dialog';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '~/components/ui/menu';
import { writeTextToClipboard } from '~/hooks/useCopyToClipboard';
import {
  PLUGIN_STATE_LABELS, SAMPLE_PLUGINS, pluginTone,
  type PluginState, type SamplePlugin,
} from './plugins-sample';
import './plugins.css';

export type PluginVariant = 'A' | 'B' | 'C';
const VARIANT_KEY = 'hoh:plugins-design-variant';

export function parsePluginVariant(value: string | null): PluginVariant {
  return value === 'B' || value === 'C' ? value : 'A';
}

function storedVariant(): PluginVariant {
  try { return parsePluginVariant(window.localStorage.getItem(VARIANT_KEY)); }
  catch { return 'A'; }
}

function FolderTile({ plugin }: { plugin: SamplePlugin }) {
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

function LanguageBadges({ plugin }: { plugin: SamplePlugin }) {
  return <span className="hoh-plugin-languages">{plugin.languages.map((language) => <Badge key={language} variant="outline">{language}</Badge>)}</span>;
}

function PluginIdentity({ plugin, compact = false }: { plugin: SamplePlugin; compact?: boolean }) {
  return (
    <div className="hoh-plugin-identity" data-compact={compact || undefined}>
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
  plugin: SamplePlugin;
  embedded?: boolean;
  copied: string | null;
  copyError: string | null;
  onBack?: () => void;
  onOpenFull?: () => void;
  onCopy: (label: string, value: string) => void;
  onRequestState: (id: string) => void;
  onRequestRemove: (plugin: SamplePlugin) => void;
}

function PluginDetail({ plugin, embedded, copied, copyError, onBack, onOpenFull, onCopy, onRequestState, onRequestRemove }: DetailProps) {
  const shortFingerprint = `${plugin.fingerprint.slice(0, 19)}…${plugin.fingerprint.slice(-8)}`;
  const action = plugin.state === 'enabled' ? 'Disable' : 'Enable';
  const blocked = plugin.state === 'blocked';
  return (
    <article className="hoh-plugin-detail" data-plugin-detail={plugin.id} data-embedded={embedded || undefined}>
      <header className="hoh-plugin-detail__header">
        {!embedded && onBack ? <Button variant="ghost" size="sm" onClick={onBack}><ArrowLeftIcon />All plugins</Button> : null}
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
          {embedded && onOpenFull ? <Button className="hoh-plugin-detail__full" size="sm" variant="ghost" onClick={onOpenFull}>Open full page<ChevronRightIcon /></Button> : null}
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

function CardGrid({ plugins, onOpen }: { plugins: readonly SamplePlugin[]; onOpen: (plugin: SamplePlugin) => void }) {
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

function ExpandableList({ plugins, expanded, onExpand, onOpen }: { plugins: readonly SamplePlugin[]; expanded: string | null; onExpand: (id: string) => void; onOpen: (plugin: SamplePlugin) => void }) {
  return (
    <div className="hoh-plugin-rows">
      {plugins.map((plugin) => {
        const open = expanded === plugin.id;
        const panelId = `plugin-row-${plugin.id}`;
        return (
          <article key={plugin.id} className="hoh-plugin-row" data-state={plugin.state} data-plugin-id={plugin.id}>
            <button type="button" className="hoh-plugin-row__trigger" aria-expanded={open} aria-controls={panelId} onClick={() => onExpand(plugin.id)}>
              <PluginIdentity plugin={plugin} compact />
              <span className="hoh-plugin-row__meta"><StatusBadge state={plugin.state} /><LanguageBadges plugin={plugin} />{open ? <ChevronDownIcon /> : <ChevronRightIcon />}</span>
            </button>
            {open ? (
              <div className="hoh-plugin-row__panel" id={panelId}>
                <p>{plugin.summary}</p>
                <FactList title="Adds" values={plugin.contributions} />
                <FactList title="Permissions and data" values={plugin.permissions} />
                <Button size="sm" variant="outline" onClick={() => onOpen(plugin)}>Open plugin page<ChevronRightIcon /></Button>
              </div>
            ) : null}
          </article>
        );
      })}
    </div>
  );
}

function MasterList({ plugins, selectedId, onSelect }: { plugins: readonly SamplePlugin[]; selectedId: string; onSelect: (plugin: SamplePlugin) => void }) {
  return (
    <div className="hoh-plugin-master">
      {plugins.map((plugin) => (
        <button key={plugin.id} type="button" className="hoh-plugin-master__item" data-state={plugin.state} data-selected={plugin.id === selectedId || undefined} aria-pressed={plugin.id === selectedId} onClick={() => onSelect(plugin)}>
          <PluginIdentity plugin={plugin} compact />
          <StatusBadge state={plugin.state} />
        </button>
      ))}
    </div>
  );
}

function GroupHeading({ builtIn }: { builtIn: boolean }) {
  return <div className="hoh-plugin-group-title"><h2>{builtIn ? 'Built-in plugins' : 'Installed plugins'}</h2><span>{builtIn ? 'Shipped with HoH' : 'Added to this installation'}</span></div>;
}

export function PluginsPage() {
  const [variant, setVariant] = useState<PluginVariant>(storedVariant);
  const [statuses, setStatuses] = useState<Partial<Record<string, PluginState>>>({});
  const [selectedId, setSelectedId] = useState(SAMPLE_PLUGINS.find((plugin) => !plugin.builtIn)?.id ?? SAMPLE_PLUGINS[0].id);
  const [detailMode, setDetailMode] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [folderDialog, setFolderDialog] = useState(false);
  const [removePlugin, setRemovePlugin] = useState<SamplePlugin | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [copyError, setCopyError] = useState<string | null>(null);
  const plugins = SAMPLE_PLUGINS.map((plugin) => ({ ...plugin, state: statuses[plugin.id] ?? plugin.state }));
  const installed = plugins.filter((plugin) => !plugin.builtIn);
  const builtIn = plugins.filter((plugin) => plugin.builtIn);
  const selected = plugins.find((plugin) => plugin.id === selectedId) ?? plugins[0];

  const chooseVariant = (next: PluginVariant) => {
    setVariant(next); setDetailMode(false); setExpanded(null);
    try { window.localStorage.setItem(VARIANT_KEY, next); } catch { /* blocked storage must not break the page */ }
  };
  const openPlugin = (plugin: SamplePlugin) => {
    document.querySelector('.hoh-page')?.scrollTo({ top: 0 });
    setSelectedId(plugin.id); setDetailMode(true); setCopied(null); setCopyError(null);
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
    <main className="hoh-plugins" data-plugin-variant={variant}>
      <header className="hoh-plugins__top">
        <div><span className="hoh-plugins__kicker">Extensions</span><h1>Plugins</h1><p>Tools, skills, pages, and controls that extend HoH.</p></div>
        <Button variant="outline" onClick={() => setFolderDialog(true)} data-add-plugin-folder><FolderPlusIcon />Add from folder</Button>
      </header>
      <div className="hoh-plugins__sample" role="note"><PackageOpenIcon /><strong>Sample data</strong><span>— not connected to the engine yet.</span></div>
      <div className="hoh-plugins__round">
        <span>Temporary · design round</span>
        <div role="group" aria-label="Plugin page design variant">
          {(['A', 'B', 'C'] as const).map((item) => <Button key={item} size="xs" variant={variant === item ? 'secondary' : 'ghost'} aria-pressed={variant === item} onClick={() => chooseVariant(item)}>Variant {item}</Button>)}
        </div>
      </div>

      {detailMode ? <PluginDetail plugin={selected} onBack={() => setDetailMode(false)} {...detailProps} /> : variant === 'A' ? (
        <div className="hoh-plugins__groups" data-plugin-list>
          <section><GroupHeading builtIn={false} /><CardGrid plugins={installed} onOpen={openPlugin} /></section>
          <section><GroupHeading builtIn /><CardGrid plugins={builtIn} onOpen={openPlugin} /></section>
        </div>
      ) : variant === 'B' ? (
        <div className="hoh-plugins__groups" data-plugin-list>
          <section><GroupHeading builtIn={false} /><ExpandableList plugins={installed} expanded={expanded} onExpand={(id) => setExpanded(expanded === id ? null : id)} onOpen={openPlugin} /></section>
          <section><GroupHeading builtIn /><ExpandableList plugins={builtIn} expanded={expanded} onExpand={(id) => setExpanded(expanded === id ? null : id)} onOpen={openPlugin} /></section>
        </div>
      ) : (
        <div className="hoh-plugin-split" data-plugin-list>
          <aside>
            <GroupHeading builtIn={false} /><MasterList plugins={installed} selectedId={selected.id} onSelect={(plugin) => setSelectedId(plugin.id)} />
            <GroupHeading builtIn /><MasterList plugins={builtIn} selectedId={selected.id} onSelect={(plugin) => setSelectedId(plugin.id)} />
          </aside>
          <PluginDetail plugin={selected} embedded onOpenFull={() => openPlugin(selected)} {...detailProps} />
        </div>
      )}

      <Dialog open={folderDialog} onOpenChange={setFolderDialog}>
        <DialogPopup>
          <DialogHeader><DialogTitle>Add plugin from folder</DialogTitle><DialogDescription>This design-round control is not connected to the system folder picker yet.</DialogDescription></DialogHeader>
          <DialogPanel><p className="hoh-plugin-dialog-note">A real folder will be inspected, quarantined, and shown for approval before any plugin can run.</p></DialogPanel>
          <DialogFooter><DialogClose render={<Button variant="outline" />}>Close</DialogClose><DialogAction disabled>Choose folder</DialogAction></DialogFooter>
        </DialogPopup>
      </Dialog>
      <Dialog open={removePlugin !== null} onOpenChange={(open) => { if (!open) setRemovePlugin(null); }}>
        <DialogPopup>
          <DialogHeader><DialogTitle>Remove {removePlugin?.name}?</DialogTitle><DialogDescription>The real action will open a system confirmation window.</DialogDescription></DialogHeader>
          <DialogPanel><p className="hoh-plugin-dialog-note">For this sample, requesting removal only moves the plugin to “Approval pending”. No files are changed.</p></DialogPanel>
          <DialogFooter>
            <DialogClose render={<Button variant="outline" />}>Cancel</DialogClose>
            <DialogAction onClick={() => { if (removePlugin) requestState(removePlugin.id); setRemovePlugin(null); }}>Request removal</DialogAction>
          </DialogFooter>
        </DialogPopup>
      </Dialog>
    </main>
  );
}
