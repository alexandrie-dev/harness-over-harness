// Прослойка окна HoH вместо apps/web/src/assets/assetUrls.ts (T3 Code, MIT): ресурсов сервера T3 у окна нет,
// адрес для них всегда «недоступно».
import type { AssetResource, EnvironmentId } from '@t3tools/contracts';

export type AssetUrlState =
  | { readonly _tag: 'Loading' }
  | { readonly _tag: 'Success'; readonly url: string; readonly imageDimensions?: { readonly width: number; readonly height: number } }
  | { readonly _tag: 'Failure'; readonly reason?: 'missing-file-access' | 'unavailable' };

const UNAVAILABLE: AssetUrlState = { _tag: 'Failure', reason: 'unavailable' };
export function useAssetUrlState(_environmentId: EnvironmentId | null, _resource: AssetResource | null, _revision?: string | null): AssetUrlState { return UNAVAILABLE; }
export function useAssetUrlRefresh(_environmentId: EnvironmentId | null, _resource: AssetResource | null): () => Promise<string | null> { return async () => null; }
