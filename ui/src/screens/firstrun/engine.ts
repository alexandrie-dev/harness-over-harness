// Единственное место, где первый запуск говорит с движком.
// Тип запросов — из ui/src/protocol.ts (порождается из hoh-protocol, здесь не правится).
import { invoke } from '@tauri-apps/api/core';
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

async function call<M extends Method>(method: M, params: MethodMap[M]['params']): Promise<MethodMap[M]['result']> {
  const hook = window.__HOH_ENGINE__;
  if (hook) return (await hook.request(method, params)) as MethodMap[M]['result'];
  if (window.__TAURI_INTERNALS__) return invoke<MethodMap[M]['result']>('engine_request', { method, params });
  throw new Error('нет связи с движком');
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
