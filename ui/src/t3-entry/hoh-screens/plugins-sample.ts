// HOH-42 design-round fixture. This is deliberately the only source of plugin sample data;
// the future engine adapter can replace this module without changing the three presentations.
export type PluginLanguage = 'MD' | 'TS' | 'Rust' | 'Go';
export type PluginState = 'enabled' | 'quarantined' | 'update-pending' | 'blocked' | 'disabled' | 'approval-pending';

export interface SamplePlugin {
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

export const SAMPLE_PLUGINS: readonly SamplePlugin[] = [
  {
    id: 'alexandrie-orbit-tools', name: 'Orbit Tools', publisher: 'Alexandrie', version: '1.4.0',
    languages: ['Rust'], state: 'enabled', builtIn: true,
    summary: 'Small, focused tools for inspecting the active workspace.',
    contributions: ['3 tools', '1 toolbar button'], replacements: [],
    permissions: ['Reads files you choose in the current project', 'Runs only its bundled helper process'],
    fingerprint: 'sha256:3d7f6bf47d824da418d43fbba0628dce3f7825688b364a9a292485cab13ed02b',
  },
  {
    id: 'alexandrie-memory-loom', name: 'Memory Loom', publisher: 'Alexandrie', version: '1.1.2',
    languages: ['MD'], state: 'disabled', builtIn: true,
    summary: 'Reusable writing and recall skills for long-running work.',
    contributions: ['4 skills'], replacements: [],
    permissions: ['Can add instructions only when you choose one of its skills', 'Cannot run programs or use the network'],
    fingerprint: 'sha256:850fc573ab2b01928de4032c5e54720d8d5d13c8b5323223e7fb6f8377889081',
  },
  {
    id: 'alexandrie-canvas-dock', name: 'Canvas Dock', publisher: 'Alexandrie', version: '2.0.0', previousVersion: '1.8.3',
    languages: ['TS', 'Rust'], state: 'update-pending', builtIn: true,
    summary: 'A visual page and quick action for inspecting generated artifacts.',
    contributions: ['1 page', '1 sidebar button'], replacements: ['Artifact preview page'],
    permissions: ['Reads artifacts created in this conversation', 'Shows its page in an isolated view'],
    fingerprint: 'sha256:aa90e2dc604733088f79d75e7f5674e098f14b3b63550a0a507cc7d28f951917',
  },
  {
    id: 'northlight-draftsmith', name: 'Draftsmith', publisher: 'Northlight Works', version: '0.9.1',
    languages: ['MD'], state: 'quarantined', builtIn: false,
    summary: 'Editing skills for briefs, release notes, and product copy.',
    contributions: ['6 skills'], replacements: [],
    permissions: ['Can read the text you give to an active skill', 'Cannot read files, run programs, or use the network'],
    fingerprint: 'sha256:17417112f7ba6b3cb075988c2072be80d66153c93c276f5a3e6010f8353e717a',
  },
  {
    id: 'bluehour-signal-relay', name: 'Signal Relay', publisher: 'Bluehour Lab', version: '1.3.5',
    languages: ['Go'], state: 'enabled', builtIn: false,
    summary: 'Sends approved summaries to a team endpoint.',
    contributions: ['2 tools', '1 composer button'], replacements: [],
    permissions: ['Reads the approved summary, not the full conversation', 'Connects only to relay.bluehour.example'],
    fingerprint: 'sha256:570b7da6230a24ad656447b6810cb4660a8fab2951190087f71a1e5d2441176d',
  },
  {
    id: 'copperline-workspace-bridge', name: 'Workspace Bridge', publisher: 'Copperline Studio', version: '0.6.0',
    languages: ['Rust'], state: 'blocked', builtIn: false,
    summary: 'A broad workspace connector blocked by policy.',
    contributions: ['5 tools', '2 hooks'], replacements: ['Core file reader'],
    permissions: ['Requested all-project file access', 'Requested connections to any network address', 'Requested permission to replace a core tool'],
    fingerprint: 'sha256:c1636c9bc4b2291b156859973065296d3f3e7dcd75b7b17123c121c3ea4bac16',
  },
  {
    id: 'softarc-quiet-hours', name: 'Quiet Hours', publisher: 'Softarc Collective', version: '1.0.4',
    languages: ['TS'], state: 'disabled', builtIn: false,
    summary: 'A calm schedule page and a pause button for non-urgent activity.',
    contributions: ['1 page', '1 sidebar button'], replacements: [],
    permissions: ['Shows its own isolated page', 'Can request a schedule change for your approval'],
    fingerprint: 'sha256:dd455088a966b235758e59d64527b75e408cd8c02c1f24a6bd9aa77fdf5bf177',
  },
] as const;

export const PLUGIN_STATE_LABELS: Readonly<Record<PluginState, string>> = {
  enabled: 'Enabled', quarantined: 'Quarantined · approval needed',
  'update-pending': 'Update approval needed', blocked: 'Blocked', disabled: 'Disabled',
  'approval-pending': 'Approval pending',
};

export function pluginTone(id: string): number {
  let hash = 0;
  for (const character of id) hash = (hash * 31 + character.charCodeAt(0)) >>> 0;
  return hash % 6;
}
