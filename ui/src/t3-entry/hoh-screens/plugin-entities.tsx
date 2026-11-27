import type { ReactNode } from 'react';
import { ClaudeAI, OllamaIcon, OpenAI, OpenRouterIcon } from '../../screens/shared/providerIcons';
import { icon as gitHubArtwork } from '../../t3/base/source-control-github/shared/icon';

const aliases = {
  '4o image generation': 'openai',
  anthropic: 'anthropic',
  chatgpt: 'openai',
  claude: 'claude',
  codex: 'openai',
  elevenlabs: 'elevenlabs',
  'eleven labs': 'elevenlabs',
  'eleven v3': 'elevenlabs',
  gemini: 'gemini',
  github: 'github',
  'gpt-4o': 'openai',
  notion: 'notion',
  ollama: 'ollama',
  openai: 'openai',
  openrouter: 'openrouter',
  postmark: 'postmark',
  postmarkapp: 'postmark',
  rust: 'rust',
  stripe: 'stripe',
  'whisper-daemon': 'openai',
  whisper: 'openai',
  'youtu.be': 'youtube',
  youtube: 'youtube',
} as const;

type Entity = (typeof aliases)[keyof typeof aliases];

const names = Object.keys(aliases).map((name) => name.replaceAll('.', '\\.')).join('|');
const entityPattern = new RegExp(`(?<![\\p{L}\\p{N}])(?:${names})(?![\\p{L}\\p{N}])`, 'giu');
const entityFor = (name: string) => aliases[name.toLowerCase() as keyof typeof aliases];

export function pluginEntity(...values: string[]): Entity | undefined {
  for (const value of values) {
    const match = value.match(entityPattern)?.[0];
    if (match) return entityFor(match);
  }
  return undefined;
}

const componentLogos = {
  claude: ClaudeAI,
  ollama: OllamaIcon,
  openai: OpenAI,
  openrouter: OpenRouterIcon,
} as const;

export function EntityLogo({ entity }: { entity: Entity }) {
  const Component = componentLogos[entity as keyof typeof componentLogos];
  return (
    <span className="hoh-plugin-logo" data-entity={entity} aria-hidden="true">
      {Component ? <Component /> : entity === 'github' ? (
        <svg viewBox={gitHubArtwork.viewBox}>
          {gitHubArtwork.paths.map(path => <path key={path.d} d={path.d} fill="currentColor" fillRule={path.fillRule} />)}
        </svg>
      ) : null}
    </span>
  );
}

export function entityText(value: unknown, leadingEntity?: Entity): ReactNode {
  if (typeof value !== 'string') return typeof value === 'number' ? value : null;
  const parts: ReactNode[] = [];
  let offset = 0;
  let skip = leadingEntity;
  for (const match of value.matchAll(entityPattern)) {
    const index = match.index;
    const name = match[0];
    const entity = entityFor(name);
    if (entity === skip) {
      skip = undefined;
      continue;
    }
    parts.push(value.slice(offset, index), (
      <span key={index} className="hoh-plugin-entity"><EntityLogo entity={entity} />{name}</span>
    ));
    offset = index + name.length;
  }
  return parts.length ? [...parts, value.slice(offset)] : value;
}
