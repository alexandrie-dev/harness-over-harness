import { OLLAMA_PORT, PROVIDERS } from './providers.data.mjs';

// Определение поставщика модели по адресу и имени. Только разбор строки: никаких обращений в сеть.

/** @returns {'loopback' | 'private' | ''} */
export function addressScope(hostname) {
  const host = hostname.toLowerCase().replace(/^\[|\]$/g, '');
  if (host === 'localhost' || host.endsWith('.localhost') || host === '::1' || /^127(\.\d{1,3}){3}$/.test(host)) return 'loopback';
  const v4 = /^(\d{1,3})\.(\d{1,3})\.\d{1,3}\.\d{1,3}$/.exec(host);
  if (v4) {
    const [a, b] = [Number(v4[1]), Number(v4[2])];
    if (a === 10 || (a === 192 && b === 168) || (a === 172 && b >= 16 && b <= 31) || (a === 169 && b === 254) || (a === 100 && b >= 64 && b <= 127)) return 'private';
    return '';
  }
  if (host.includes(':')) return /^(fc|fd|fe80)/.test(host) ? 'private' : '';
  if (host.endsWith('.local') || host.endsWith('.lan') || host.endsWith('.internal') || host.endsWith('.home.arpa') || !host.includes('.')) return 'private';
  return '';
}

/**
 * Who answers: `{ id, name, icon, scope, label }`.
 * scope — 'loopback' | 'private' | 'cloud' | 'unknown'; icon — ключ значка T3 или null (нейтральный);
 * label — то, что показывается человеку: «Локальная модель» для этого компьютера и частной сети, иначе имя модели или поставщика.
 */
export function detectProvider(baseUrl, model = '') {
  const name = String(model ?? '').trim();
  const lower = name.toLowerCase().replace(/^.*\//, '');
  let url = null;
  try { url = new URL(String(baseUrl ?? '').trim()); } catch { /* an address that is not a URL is unknown */ }
  const host = url ? url.hostname.toLowerCase() : '';
  const scope = host ? addressScope(host) : '';
  if (scope === 'loopback' || scope === 'private') {
    const ollama = url?.port === OLLAMA_PORT;
    return { id: ollama ? 'ollama' : 'local', name: 'Модель на этом компьютере', icon: ollama ? 'ollama' : 'local', scope, label: scope === 'private' ? 'Модель на другом компьютере в вашей сети' : 'Модель на этом компьютере' };
  }
  const byHost = host && PROVIDERS.find(p => p.hosts.some(h => host === h || host.endsWith('.' + h)));
  const byModel = lower && PROVIDERS.find(p => p.models.some(prefix => lower.startsWith(prefix)));
  // An address of a known service wins; the model name decides for relays that serve other vendors' models.
  const found = byHost && byHost.id === 'anthropic' && byModel?.id === 'claude' ? byModel : byHost || byModel;
  if (found) return { id: found.id, name: found.name, icon: found.icon, scope: 'cloud', label: name || found.name };
  return { id: 'unknown', name: host || 'Модель', icon: null, scope: host ? 'cloud' : 'unknown', label: name || host || 'Модель' };
}
