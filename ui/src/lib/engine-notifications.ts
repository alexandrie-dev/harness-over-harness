import { Channel, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

const listeners = new Set<(event: unknown) => void>();
let subscription: Promise<void> | undefined;

/** The native window has one ordered channel. Consumers can change independently of the bridge. */
export async function subscribeEngineNotifications<T>(listener: (event: T) => void): Promise<() => void> {
  const receive = (event: unknown) => listener(event as T);
  const unsubscribe = () => { listeners.delete(receive); };
  listeners.add(receive);
  subscription ??= (async () => {
    const deliver = (event: unknown) => { for (const listener of listeners) listener(event); };
    try {
      await invoke('subscribe_engine_notifications', { onMessage: new Channel<unknown>(deliver) });
    } catch {
      await listen('engine-notification', event => deliver(event.payload));
    }
  })().catch(error => { subscription = undefined; throw error; });
  try { await subscription; }
  catch (error) { unsubscribe(); throw error; }
  return unsubscribe;
}
