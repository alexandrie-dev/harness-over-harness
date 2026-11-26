import { ArrowUp, Folder, FolderPlus, Puzzle } from 'lucide-react';
import { useEffect, useRef, useState, type FormEvent } from 'react';
import { ProviderMark } from '../../screens/shared/ProviderMark';
import {
  MAX_PLUGIN_BRIEF, type PluginCreatorApi, type PluginCreatorOptions,
} from '../../hoh-host/plugin-creator';
import { Select, SelectItem, SelectPopup, SelectTrigger } from '../../t3/base/web/components/ui/select';
import { ComposerSpeechButton, ComposerSpeechRecordingPill } from '~/components/chat/ComposerSpeechButton';
import { useWhisperDictation } from './whisper/useComposerSpeechInput';
import './plugin-creator.css';

const starters = [
  'Summarize session activity',
  'Add a command for my team',
  'Show a result in the chat panel',
];

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
  const composer = useRef<HTMLTextAreaElement>(null);
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

  const available = options?.harnesses.filter(harness => harness.available) ?? [];
  const selected = options?.harnesses.find(harness => harness.executor === executor)
    ?? (available.length === 1 ? available[0] : undefined);
  const project = options?.projects.find(project => project.id === projectId)
    ?? (options?.projects.length === 1 ? options.projects[0] : undefined);
  const voice = useWhisperDictation({
    ownerKey: 'plugin-creator-brief',
    disabled: pending || !options,
    readDraft: () => ({ text: brief }),
    commitDraft: text => {
      if (text.length > MAX_PLUGIN_BRIEF) {
        setError(`The plugin request cannot exceed ${MAX_PLUGIN_BRIEF.toLocaleString()} characters.`);
        return;
      }
      setBrief(text);
      const editor = composer.current;
      window.requestAnimationFrame(() => {
        if (editor && composer.current === editor) {
          editor.focus();
          editor.setSelectionRange(text.length, text.length);
        }
      });
    },
  });
  const canLaunch = !!(creator && project && selected?.available && brief.trim() && !pending && !voice.blocksSubmission);

  function startDictation() {
    if (!voice.configured) {
      setError('Set the Whisper server address in Settings → Chat.');
      return;
    }
    setError('');
    void voice.start();
  }

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
    <main className="hoh-plugin-creator" aria-labelledby="plugin-creator-title">
      <header className="hoh-plugin-creator__header">
        <Puzzle size={16} aria-hidden />
        <span>Plugin Creator</span>
      </header>
      <div className="hoh-plugin-creator__lane">
        <section className="hoh-plugin-creator__intro">
          <div className="hoh-plugin-creator__mark" aria-hidden><Puzzle size={27} strokeWidth={1.5} /></div>
          <h1 id="plugin-creator-title">What should your plugin do?</h1>
          <p>Your harness will ask questions and confirm the plan before building.</p>
          {options && !brief && !pending ? (
            <div className="hoh-plugin-creator__starters" aria-label="Example requests">
              {starters.map(starter => (
                <button type="button" key={starter} onClick={() => { setBrief(starter); composer.current?.focus(); }}>
                  {starter}
                </button>
              ))}
            </div>
          ) : null}
        </section>

        {loadError ? <p role="alert">{loadError}</p> : !options ? <p role="status">Loading projects and harnesses…</p> : (
          <form className="hoh-plugin-creator__composer" onSubmit={submit}
            onKeyDown={event => {
              if (event.key === 'Escape' && voice.blocksSubmission) {
                event.preventDefault();
                event.stopPropagation();
                voice.cancel();
              }
            }}>
            <textarea id="plugin-creator-brief" ref={composer} rows={3} aria-label="Plugin request"
              maxLength={MAX_PLUGIN_BRIEF} value={brief} disabled={pending} readOnly={voice.freezesEditor}
              placeholder="Describe the plugin you want to build…"
              onChange={event => setBrief(event.target.value)}
              onKeyDown={event => {
                if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                  event.preventDefault();
                  if (!voice.blocksSubmission) event.currentTarget.form?.requestSubmit();
                }
              }} />
            <div className="hoh-plugin-creator__toolbar">
              <div className="hoh-plugin-creator__options">
                {options.projects.length ? (
                  <Select value={project?.id ?? null} disabled={pending}
                    onValueChange={value => setProjectId(value ?? '')}>
                    <SelectTrigger id="plugin-creator-project" className="hoh-plugin-creator__picker"
                      variant="ghost" size="sm" aria-label="Plugin folder" title={project?.path}>
                      <Folder size={16} aria-hidden />
                      <span className="truncate">{project?.title ?? 'Choose a folder'}</span>
                    </SelectTrigger>
                    <SelectPopup alignItemWithTrigger={false} className="hoh-plugin-creator__picker-list">
                      {options.projects.map(option => (
                        <SelectItem key={option.id} value={option.id} label={option.title} title={option.path}>
                          <span className="block truncate">{option.title}</span>
                        </SelectItem>
                      ))}
                    </SelectPopup>
                  </Select>
                ) : null}
                <button type="button" className="hoh-plugin-creator__add" aria-label="Add plugin folder"
                  title="Add plugin folder" disabled={pending} onClick={onAddProject}>
                  <FolderPlus size={16} aria-hidden />
                  {!options.projects.length ? 'Add folder' : null}
                </button>
                <Select value={selected?.executor ?? null} disabled={pending}
                  onValueChange={value => setExecutor(value ?? '')}>
                  <SelectTrigger id="plugin-creator-harness" className="hoh-plugin-creator__picker"
                    variant="ghost" size="sm" aria-label="Harness" title={selected?.model}>
                    {selected ? <ProviderMark icon={selected.executor === 'codex' ? 'openai' : 'claude'} /> : <Puzzle size={16} aria-hidden />}
                    <span className="truncate">{selected?.name ?? 'Choose a harness'}</span>
                  </SelectTrigger>
                  <SelectPopup alignItemWithTrigger={false} className="hoh-plugin-creator__picker-list">
                    {options.harnesses.map(harness => (
                      <SelectItem key={harness.executor} value={harness.executor} label={harness.name}
                        disabled={!harness.available}>
                        <span className="flex items-center gap-2">
                          <ProviderMark icon={harness.executor === 'codex' ? 'openai' : 'claude'} />
                          <span>{harness.name}{harness.available ? '' : ' · Unavailable'}</span>
                        </span>
                      </SelectItem>
                    ))}
                  </SelectPopup>
                </Select>
              </div>
              <div className="hoh-plugin-creator__actions">
                {voice.available && (
                  <>
                    <ComposerSpeechRecordingPill state={voice.state} progress={null} level={voice.level}
                      recordingStartedAt={voice.recordingStartedAt} cancelShortcutLabel="Esc"
                      disabled={pending} onStart={startDictation} onStop={() => void voice.stop()}
                      onCancel={voice.cancel} onSkipPostProcessing={() => {}} />
                    <ComposerSpeechButton state={voice.state} disabled={pending} onStart={startDictation} />
                  </>
                )}
                <button type="submit" className="hoh-plugin-creator__send" aria-label="Send plugin request"
                  title="Send plugin request" disabled={!canLaunch}>
                  <ArrowUp size={19} aria-hidden />
                </button>
              </div>
            </div>
            {pending ? <p role="status">Opening {selected?.name} chat…</p> : null}
            {voice.whisperError ? <p role="alert">{voice.whisperError}</p> : null}
            {error ? <p role="alert">{error}</p> : null}
            {!pending && !error && (selected?.available === false || !available.length) ? (
              <p role="status">{selected?.reason ?? options.harnesses.find(harness => harness.reason)?.reason ?? 'Connect a harness to send your request.'}</p>
            ) : null}
          </form>
        )}
      </div>
    </main>
  );
}
