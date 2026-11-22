// Единственное место, где первый запуск говорит с движком.
// Тип запросов — из ui/src/protocol.ts (порождается из hoh-protocol, здесь не правится).
import { invoke } from '@tauri-apps/api/core';
import type { DiscoverResult } from '../../lib/executor.mjs';
import type { MethodMap, ModelProbeResult, ModelSettings } from '../../protocol';

type Method = keyof MethodMap;

/** Метода нет в протоколе окна (docs/window.md §4). Не заменять догадкой — сообщить ведущему. */
export class MethodMissing extends Error {
  constructor(readonly what: string) {
    super(`в протоколе окна нет метода: ${what}`);
  }
}

// Проверки подставляют свой транспорт через window.__HOH_ENGINE__ (вне Tauri).
// В сборке с Tauri запрос идёт командой ядра `engine_request`, которая отдаёт его
// движку по stdio; имя и форма закреплены в docs/window.md §4; реализация — пакет 4 (apps/hoh-desktop/src/bridge.rs).
interface EngineHook {
  request(method: string, params: unknown): Promise<unknown>;
  // Необязательные возможности ядра, которых нет в протоколе: проверки подставляют их сами.
  storeApiKey?(apiKey: string, baseUrl: string): Promise<void>;
  saveRules?(rules: Rules): Promise<void>;
  pickFolder?(): Promise<string | null>;
}
declare global { interface Window { __HOH_ENGINE__?: EngineHook; __TAURI_INTERNALS__?: unknown } }

async function send(method: string, params: unknown): Promise<unknown> {
  const hook = window.__HOH_ENGINE__;
  if (hook) return hook.request(method, params);
  if (window.__TAURI_INTERNALS__) return invoke('engine_request', { method, params });
  throw new Error('нет связи с Помощником');
}
async function call<M extends Method>(method: M, params: MethodMap[M]['params']): Promise<MethodMap[M]['result']> {
  return (await send(method, params)) as MethodMap[M]['result'];
}

/** `hoh/executor/discover` (контракт движка; в сгенерированном протоколе окна его пока нет): только чтение, настоящая проверка при каждом вызове. */
export async function discoverClaude(): Promise<DiscoverResult> {
  return (await send('hoh/executor/discover', { executor: 'claude' })) as DiscoverResult;
}

/** `hoh/executor/access/get`: как движок запустит Claude в следующий раз (`effective.flags`). Не ответил — `undefined`: окно ничего не домысливает. */
export async function claudeLaunchFlags(): Promise<unknown> {
  try {
    const answer = (await send('hoh/executor/access/get', { executor: 'claude' })) as { effective?: { flags?: unknown } } | null;
    return answer?.effective?.flags;
  } catch { return undefined; }
}

/** `hoh/executor/select`: привилегированная, хост спрашивает человека. После неё новые чаты идут через Claude с этой моделью. */
export async function selectClaude(model: string): Promise<void> {
  await send('hoh/executor/select', { executor: 'claude', model });
}

/** Настройки модели, уже записанные в движке (адрес и название), чтобы заполнить поля. */
export async function loadModelSettings(): Promise<ModelSettings | null> {
  try {
    return (await call('hoh/settings/get', {})).model;
  } catch {
    return null;
  }
}

/**
 * API-ключ уходит движку и хранится через hoh-credentials; в окне он живёт только в поле ввода.
 * engine_request запрещено журналировать параметры этого метода; подтверждение содержит keyHash и baseUrl.
 */
export async function storeApiKey(apiKey: string, baseUrl: string): Promise<void> {
  const hook = window.__HOH_ENGINE__;
  if (hook?.storeApiKey) return hook.storeApiKey(apiKey, baseUrl);
  await call('hoh/model/key/set', { apiKey, baseUrl });
}

export function probeModel(baseUrl: string, model: string): Promise<ModelProbeResult> {
  return call('hoh/model/probe', { baseUrl, model });
}

/** Compatibility boundary while ui1 adds models to the generated result. */
export function modelsFromProbe(result: ModelProbeResult): string[] {
  const models = (result as ModelProbeResult & { models?: unknown }).models;
  return Array.isArray(models) ? models.filter((value): value is string => typeof value === 'string' && !!value.trim()).map(value => value.trim()) : [];
}

export async function saveModel(baseUrl: string, model: string): Promise<void> {
  await call('hoh/settings/set', { model: { baseUrl, model } });
}

export interface Rules { internet: 'ask' | 'public'; folder: string }

/** Режим интернета и папка: отдельное подписываемое действие первого запуска. */
export async function saveRules(rules: Rules): Promise<void> {
  const hook = window.__HOH_ENGINE__;
  if (hook?.saveRules) return hook.saveRules(rules);
  await call('hoh/settings/rules/set', rules);
}

/** Системный выбор папки. Окно файловую систему не видит; нужен диалог в ядре Tauri. */
export async function pickFolder(): Promise<string | null> {
  const hook = window.__HOH_ENGINE__;
  if (hook?.pickFolder) return hook.pickFolder();
  if (window.__TAURI_INTERNALS__) return invoke<string | null>('pick_folder');
  throw new MethodMissing('выбор папки (нет связи с ядром)');
}

/** The folder the engine's session works in now (the default one on a first run), as a real path; null when unknown. */
export async function loadWorkFolder(): Promise<string | null> {
  try {
    const answer = await call('session/working-directory/get', { sessionId: 'window-main' });
    return typeof answer.cwd === 'string' && answer.cwd.startsWith('/') ? answer.cwd : null;
  } catch { return null; }
}
