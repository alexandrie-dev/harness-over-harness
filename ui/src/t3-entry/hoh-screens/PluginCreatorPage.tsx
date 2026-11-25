import { useEffect, useRef, useState, type FormEvent } from 'react';
import {
  MAX_PLUGIN_BRIEF, type PluginCreatorApi, type PluginCreatorOptions,
} from '../../hoh-host/plugin-creator';
import './plugins.css';

interface Props {
  creator?: PluginCreatorApi;
  onAddProject(): void;
  onOpenThread(threadId: string): void;
}

export function PluginCreatorPage({
  creator = window.__HOH_PLUGIN_CREATOR__,
  onAddProject,
  onOpenThread,
}: Props) {
  const [options, setOptions] = useState<PluginCreatorOptions | null>(null);
  const [loadError, setLoadError] = useState('');
  const [error, setError] = useState('');
  const [projectId, setProjectId] = useState('');
  const [executor, setExecutor] = useState('');
  const [brief, setBrief] = useState('');
  const [pending, setPending] = useState(false);
  const launching = useRef(false);
  const mounted = useRef(false);

  useEffect(() => {
    mounted.current = true;
    let generation = 0;
    const refresh = async () => {
      const current = ++generation;
      try {
        if (!creator) throw new Error('Plugin Creator is unavailable.');
        const result = await creator.options();
        if (generation === current) {
          setOptions(result);
          setLoadError('');
        }
      } catch (error) {
        if (generation === current) {
          setOptions(null);
          setLoadError(error instanceof Error ? error.message : 'Could not load Plugin Creator.');
        }
      }
    };
    const unsubscribe = creator?.subscribe(() => { void refresh(); });
    void refresh();
    return () => {
      mounted.current = false;
      generation++;
      unsubscribe?.();
    };
  }, [creator]);

  const selected = options?.harnesses.find(harness => harness.executor === executor);
  const project = options?.projects.find(project => project.id === projectId);
  const canLaunch = !!(creator && project && selected?.available && brief.trim() && !pending);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!creator || !project || !selected?.available || !canLaunch || launching.current) return;
    launching.current = true;
    setPending(true);
    setError('');
    try {
      const id = await creator.launch({ projectId: project.id, executor: selected.executor, brief: brief.trim() });
      if (mounted.current) onOpenThread(id);
    } catch (error) {
      if (mounted.current) setError(error instanceof Error ? error.message : 'Could not start the harness.');
    } finally {
      launching.current = false;
      if (mounted.current) setPending(false);
    }
  }

  return (
    <main className="hoh-plugins hoh-plugin-creator">
      <header className="hoh-plugins__top">
        <div>
          <h1>Plugin Creator</h1>
          <p>Start a chat with the HoH plugin authoring guide and tests.</p>
        </div>
      </header>
      {loadError ? <p role="alert">{loadError}</p> : !options ? <p role="status">Loading projects and harnesses…</p> : (
        <form className="hoh-plugin-creator__form" onSubmit={submit}>
          <div className="hoh-plugin-creator__choices">
            <div>
              <label htmlFor="plugin-creator-project">HoH checkout</label>
              <select id="plugin-creator-project" value={projectId} disabled={pending} onChange={event => setProjectId(event.target.value)}>
                <option value="">Choose a project</option>
                {options.projects.map(project => <option key={project.id} value={project.id}>{project.title} ({project.path})</option>)}
              </select>
              <button type="button" className="hoh-plugin-creator__add" disabled={pending} onClick={onAddProject}>Add project</button>
            </div>
            <div>
              <label htmlFor="plugin-creator-harness">Harness</label>
              <select id="plugin-creator-harness" value={executor} disabled={pending} onChange={event => setExecutor(event.target.value)}>
                <option value="">Choose a harness</option>
                {options.harnesses.map(harness => (
                  <option key={harness.executor} value={harness.executor} disabled={!harness.available}>
                    {harness.name}{harness.available ? ` · ${harness.model}` : ' · Unavailable'}
                  </option>
                ))}
              </select>
              {options.harnesses.filter(harness => !harness.available && harness.reason).map(harness => (
                <p key={harness.executor}>{harness.reason}</p>
              ))}
            </div>
          </div>
          <div>
            <label htmlFor="plugin-creator-brief">What should the plugin do?</label>
            <textarea id="plugin-creator-brief" rows={4} maxLength={MAX_PLUGIN_BRIEF} value={brief} disabled={pending}
              onChange={event => setBrief(event.target.value)}
              placeholder="For example, add a tool that summarizes the latest session activity." />
          </div>
          {error ? <p role="alert">{error}</p> : null}
          <div className="hoh-plugin-creator__actions">
            <button type="submit" disabled={!canLaunch}>{pending ? 'Starting…' : 'Start authoring'}</button>
          </div>
        </form>
      )}
    </main>
  );
}
