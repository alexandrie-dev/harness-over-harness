import { Cpu, Sparkles } from 'lucide-react';
import { ClaudeAI, OllamaIcon, OpenAI, OpenRouterIcon } from './providerIcons';

const ICONS = { openai: OpenAI, claude: ClaudeAI, anthropic: ClaudeAI, openrouter: OpenRouterIcon, ollama: OllamaIcon } as const;

/** Значок поставщика модели из набора T3; ключ из lib/providers.mjs. Для локальной модели — «компьютер», для неизвестного — нейтральная искра. */
export function ProviderMark({ icon }: { icon: string }) {
  const Known = ICONS[icon as keyof typeof ICONS];
  return <span className="provider-mark" data-provider-icon={icon || 'neutral'} aria-hidden="true">
    {Known ? <Known /> : icon === 'local' ? <Cpu className="ic" /> : <Sparkles className="ic" />}
  </span>;
}
