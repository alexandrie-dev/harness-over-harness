import { invoke } from '@tauri-apps/api/core';
import { ArrowUpRight, Download, RefreshCw } from 'lucide-react';
import { useState } from 'react';
import {
  externalUiError, installExternalUiPlugin, refreshExternalUiCatalog, useExternalUiCatalog, type PagePlugin,
} from './external-ui-sdk';
import { pages, workspacePage } from './pages';
import './external-ui-plugins.css';

export function ExternalUiPlugins() {
  const { desktop, catalog, error } = useExternalUiCatalog();
  const [openError, setOpenError] = useState('');
  const [opening, setOpening] = useState<string | null>(null);
  const [installing, setInstalling] = useState<'prepared' | 'folder' | null>(null);
  const [installedName, setInstalledName] = useState('');
  if (!desktop) return null;
  const prepared = catalog?.prepared;

  const install = async (source: 'prepared' | 'folder') => {
    setInstalling(source);
    setOpenError('');
    setInstalledName('');
    try {
      const item = await installExternalUiPlugin(source === 'prepared' ? prepared : undefined);
      if (item?.workspaceLabel && pages.current() === 'plugins') pages.open(workspacePage(item.id));
      else if (item) setInstalledName(item.name);
    } catch (failure) {
      setOpenError(externalUiError(failure, 'The plugin could not be installed.'));
    } finally {
      setInstalling(null);
    }
  };

  const open = async (item: PagePlugin) => {
    if (item.workspaceLabel) {
      pages.open(workspacePage(item.id));
      return;
    }
    setOpening(item.id);
    setOpenError('');
    try {
      await invoke('open_external_ui_plugin', { package: item });
    } catch (failure) {
      setOpenError(externalUiError(failure, 'The plugin page could not be opened.'));
    } finally {
      setOpening(null);
    }
  };

  return (
    <section className="hoh-external-ui" aria-label="External UI plugins">
      <div className="hoh-external-ui__header">
        <h2>External UI plugins</h2>
        <div className="hoh-external-ui__actions">
          {catalog && <button type="button" disabled={installing !== null} onClick={() => void install('folder')}>
            <Download aria-hidden="true" /> {installing === 'folder' ? 'Installing…' : 'Install from folder'}
          </button>}
          <button type="button" disabled={installing !== null}
            onClick={() => { setOpenError(''); void refreshExternalUiCatalog(); }}>
            <RefreshCw aria-hidden="true" /> Refresh
          </button>
        </div>
      </div>
      {(openError || error) && <p role="alert">{openError || error}</p>}
      {installedName && <p role="status">{installedName} installed</p>}
      {catalog && (
        <>
          {prepared && <ul aria-label="Prepared plugin">
            <li>
              <div><strong>{prepared.name}</strong><p>{prepared.version}</p></div>
              <button type="button" aria-label={`Install ${prepared.name}`}
                disabled={installing !== null} onClick={() => void install('prepared')}>
                <Download aria-hidden="true" /> {installing === 'prepared' ? 'Installing…' : 'Install'}
              </button>
            </li>
          </ul>}
          {catalog.packages.length > 0 && <ul>
            {catalog.packages.map(item => (
              <li key={item.id}>
                <div>
                  <strong>{item.name}</strong>
                  <p>{item.id} · {item.version}</p>
                  <code title={item.digest}>{item.digest}</code>
                </div>
                {item.chatPanel ? <span>Open from Workspace</span> : <button type="button" aria-label={`Open ${item.name}`}
                  disabled={opening !== null} onClick={() => void open(item)}>
                  <ArrowUpRight aria-hidden="true" /> {opening === item.id ? 'Opening…' : 'Open'}
                </button>}
              </li>
            ))}
          </ul>}
          {catalog.rejected.length > 0 && <ul aria-label="Unavailable external plugins">
            {catalog.rejected.map(item => (
              <li key={item.id}><div><strong>{item.id}</strong><p>{item.reason}</p></div></li>
            ))}
          </ul>}
        </>
      )}
    </section>
  );
}
