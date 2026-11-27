import {
  DEFAULT_SERVER_SETTINGS,
  KeybindingRule,
  MAX_KEYBINDINGS_COUNT,
  ProviderInstanceConfigMap,
  ProviderInstanceMutation,
  ServerRemoveKeybindingInput,
  ServerSettings,
  ServerSettingsPatch,
  ServerUpsertKeybindingInput,
} from '@t3tools/contracts';
import {
  DEFAULT_KEYBINDINGS,
  compileResolvedKeybindingRule,
  compileResolvedKeybindingsConfig,
  mergeWithDefaultKeybindings,
} from '@t3tools/shared/keybindings';
import { applyServerSettingsPatch } from '@t3tools/shared/serverSettings';
import * as Schema from 'effect/Schema';
import { EXECUTOR_INSTANCES } from '../hoh-host/config.ts';
import type { HohStorage } from '../hoh-host/host.ts';

const KEYBINDINGS_STORAGE_KEY = 'hoh.keybindings.v1';
const WINDOW_PREFERENCES_STORAGE_KEY = 'hoh.window-preferences.v1';
const PROVIDER_IDS = new Set<string>(Object.values(EXECUTOR_INSTANCES));

const WindowPreferences = Schema.Struct({
  favorites: ServerSettings.fields.favorites,
  providerModelPreferences: ServerSettings.fields.providerModelPreferences,
  providerInstances: ProviderInstanceConfigMap,
  snoozePresets: ServerSettings.fields.snoozePresets,
});
export type HohWindowPreferences = typeof WindowPreferences.Type;
const WindowPreferencesPatch = Schema.Struct({
  favorites: ServerSettingsPatch.fields.favorites,
  providerModelPreferences: ServerSettingsPatch.fields.providerModelPreferences,
  migrateModelPreferences: ServerSettingsPatch.fields.migrateModelPreferences,
  setModelFavorites: ServerSettingsPatch.fields.setModelFavorites,
  setModelsHidden: ServerSettingsPatch.fields.setModelsHidden,
  setProviderModelOrder: ServerSettingsPatch.fields.setProviderModelOrder,
  snoozePresets: ServerSettingsPatch.fields.snoozePresets,
});

function readJson(storage: HohStorage | null, key: string): unknown {
  try {
    return JSON.parse(storage?.getItem(key) ?? 'null');
  } catch {
    return null;
  }
}

function saveJson(storage: HohStorage | null, key: string, value: unknown) {
  if (!storage) throw new Error('Local preferences storage is unavailable.');
  try {
    storage.setItem(key, JSON.stringify(value));
  } catch {
    throw new Error('Local preferences could not be saved.');
  }
}

const decodeRule = Schema.decodeUnknownOption(KeybindingRule);
function storedRule(value: unknown) {
  const parsed = decodeRule(value);
  return parsed._tag === 'Some' && compileResolvedKeybindingRule(parsed.value) ? parsed.value : null;
}

function ruleIdentity(rule: KeybindingRule) {
  const resolved = compileResolvedKeybindingRule(rule);
  if (!resolved) throw new Error('Enter a valid shortcut and condition.');
  return JSON.stringify(resolved);
}

function keybindingStore(storage: HohStorage | null) {
  const stored = readJson(storage, KEYBINDINGS_STORAGE_KEY);
  // Preserve entries from other versions when editing a recognized shortcut.
  let entries = Array.isArray(stored)
    ? stored.slice(-MAX_KEYBINDINGS_COUNT).map((value: unknown) => ({ value, rule: storedRule(value) }))
    : [];
  const keybindings = () => mergeWithDefaultKeybindings(compileResolvedKeybindingsConfig(
    entries.flatMap(({ rule }) => rule ? [rule] : []),
  ));
  const commit = (next: typeof entries) => {
    saveJson(storage, KEYBINDINGS_STORAGE_KEY, next.map(({ value }) => value));
    entries = next;
    return { keybindings: keybindings(), issues: [] };
  };

  return {
    keybindings,
    upsertKeybinding(input: unknown) {
      const { replace, ...rule } = Schema.decodeUnknownSync(ServerUpsertKeybindingInput)(input);
      const nextId = ruleIdentity(rule);
      const replaceId = replace ? ruleIdentity(replace) : null;
      // An unassigned rule overrides its command. Keep any other default bindings for that command.
      const siblingDefaults = rule.when === 'false' && replace
        && !entries.some(entry => entry.rule?.command === rule.command)
        ? DEFAULT_KEYBINDINGS.filter(entry => entry.command === rule.command && ruleIdentity(entry) !== replaceId)
        : [];
      const retained = entries.filter(({ rule: entry }) => {
        if (!entry) return true;
        const id = ruleIdentity(entry);
        return id !== nextId && id !== replaceId;
      });
      return commit([
        ...siblingDefaults.map(entry => ({ value: entry, rule: entry })),
        ...retained,
        { value: rule, rule },
      ].slice(-MAX_KEYBINDINGS_COUNT));
    },
    removeKeybinding(input: unknown) {
      const id = ruleIdentity(Schema.decodeUnknownSync(ServerRemoveKeybindingInput)(input));
      return commit(entries.filter(({ rule }) => rule === null || ruleIdentity(rule) !== id));
    },
  };
}

function windowPreferenceStore(storage: HohStorage | null) {
  const stored = Schema.decodeUnknownOption(WindowPreferences)(readJson(storage, WINDOW_PREFERENCES_STORAGE_KEY));
  let preferences: HohWindowPreferences = stored._tag === 'Some' ? stored.value : {
    favorites: null,
    providerModelPreferences: null,
    providerInstances: {},
    snoozePresets: DEFAULT_SERVER_SETTINGS.snoozePresets,
  };

  return {
    windowPreferences: () => preferences,
    updateWindowPreferences(patch: unknown, rawMutation: unknown) {
      const localPatch = Schema.decodeUnknownSync(WindowPreferencesPatch)(patch);
      if (rawMutation === undefined && Object.keys(localPatch).length === 0) return false;
      const { favorites, providerModelPreferences, snoozePresets } = applyServerSettingsPatch(
        { ...DEFAULT_SERVER_SETTINGS, ...preferences },
        localPatch,
      );
      const providerInstances = { ...preferences.providerInstances };
      if (rawMutation !== undefined) {
        const mutation = Schema.decodeUnknownSync(ProviderInstanceMutation)(rawMutation);
        const { instanceId } = mutation;
        if (!PROVIDER_IDS.has(instanceId) || mutation.operation === 'create') {
          throw new Error('Choose a built-in HoH harness.');
        }
        delete providerInstances[instanceId];
        if (mutation.operation === 'upsert') {
          const { driver, displayName, accentColor } = mutation.instance;
          if (String(driver) !== String(instanceId)) throw new Error('The harness driver cannot change.');
          if (displayName || accentColor) providerInstances[instanceId] = { driver, displayName, accentColor };
        }
      }
      const next = { favorites, providerModelPreferences, providerInstances, snoozePresets };
      if (JSON.stringify(next) === JSON.stringify(preferences)) return false;
      saveJson(storage, WINDOW_PREFERENCES_STORAGE_KEY, next);
      preferences = next;
      return true;
    },
  };
}

export function createHohLocalSettings(storage: HohStorage | null) {
  return { ...keybindingStore(storage), ...windowPreferenceStore(storage) };
}

export type CreateHohLocalSettings = typeof createHohLocalSettings;
