import { invoke } from '@tauri-apps/api/core';
import { ArrowUpRight, RefreshCw } from 'lucide-react';
import { useState } from 'react';
import {
  externalUiError, refreshExternalUiCatalog, useExternalUiCatalog, type PagePlugin,
} from './external-ui-sdk';
import { pages, workspacePage } from './pages';
import './external-ui-plugins.css';

export function ExternalUiPlugins() {
  const { desktop, catalog, error } = useExternalUiCatalog();
  const [openError, setOpenError] = useState('');
  const [opening, setOpening] = useState<string | null>(null);
  if (!desktop) return null;

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
        <button type="button" onClick={() => { setOpenError(''); void refreshExternalUiCatalog(); }}>
          <RefreshCw aria-hidden="true" /> Refresh
        </button>
      </div>
      {(openError || error) && <p role="alert">{openError || error}</p>}
      {catalog && (
        <>
          <p>Place a package folder in <code>{catalog.directory}</code> and refresh.</p>
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
