import { invoke, isTauri } from '@tauri-apps/api/core';
import { HOH_ENVIRONMENT_ID, type EnvironmentPlatform } from '../hoh-host/config.ts';

let enabled = false;

export function configureMacProjectPicker(platform: EnvironmentPlatform | undefined) {
  enabled = platform?.os === 'darwin' && isTauri();
}

export function macProjectPicker(environmentId: string | null): (() => Promise<string | null>) | undefined {
  if (enabled && environmentId === HOH_ENVIRONMENT_ID) return pickFolder;
}

function pickFolder() {
  return invoke<string | null>('pick_folder');
}
