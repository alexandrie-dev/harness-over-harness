// An in-memory WebSocket for T3's client. T3 builds its socket with `Socket.layerWebSocketConstructorGlobal`
// (apps/web/src/lib/runtime.ts), i.e. `new globalThis.WebSocket(url)`. `installHohWebSocket` puts this
// class there for the HoH environment's address, so the unchanged T3 client talks to the HoH host in
// the page: no network, no server, no T3 file edited.
import type { WireConnection, WireServer } from './wire.ts';

type Listener = (event: any) => void;

export class HohWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  readonly CONNECTING = 0;
  readonly OPEN = 1;
  readonly CLOSING = 2;
  readonly CLOSED = 3;

  readyState = HohWebSocket.CONNECTING;
  binaryType: 'blob' | 'arraybuffer' = 'blob';
  readonly bufferedAmount = 0;
  readonly extensions = '';
  readonly protocol = '';
  onopen: Listener | null = null;
  onmessage: Listener | null = null;
  onclose: Listener | null = null;
  onerror: Listener | null = null;
  private readonly listeners = new Map<string, Set<Listener>>();
  private readonly onceOf = new Map<Listener, Listener>();
  private connection: WireConnection | undefined;

  constructor(readonly url: string, server: WireServer) {
    // The socket opens on a later task, as a real one does: listeners are attached by then.
    setTimeout(() => {
      if (this.readyState !== HohWebSocket.CONNECTING) return;
      this.connection = server.connect(frame => {
        // Answers arrive asynchronously, never inside the client's own send().
        setTimeout(() => { if (this.readyState === HohWebSocket.OPEN) this.emit('message', { type: 'message', data: frame }); }, 0);
      });
      this.readyState = HohWebSocket.OPEN;
      this.emit('open', { type: 'open' });
    }, 0);
  }

  addEventListener(type: string, listener: Listener, options?: boolean | { readonly once?: boolean }) {
    let set = this.listeners.get(type);
    if (!set) this.listeners.set(type, set = new Set());
    if (typeof options === 'object' && options?.once) {
      const once: Listener = event => { set!.delete(once); this.onceOf.delete(listener); listener(event); };
      this.onceOf.set(listener, once);
      set.add(once);
    } else set.add(listener);
  }

  removeEventListener(type: string, listener: Listener) {
    const set = this.listeners.get(type);
    set?.delete(listener);
    const once = this.onceOf.get(listener);
    if (once) { set?.delete(once); this.onceOf.delete(listener); }
  }

  send(data: string | ArrayBufferLike | ArrayBufferView) {
    if (this.readyState !== HohWebSocket.OPEN) throw new Error('WebSocket is not open');
    const frame = typeof data === 'string' ? data : new TextDecoder().decode(data as ArrayBuffer);
    this.connection?.receive(frame);
  }

  close(code = 1000, reason = '') {
    if (this.readyState === HohWebSocket.CLOSED) return;
    this.readyState = HohWebSocket.CLOSED;
    this.connection?.close();
    this.connection = undefined;
    setTimeout(() => this.emit('close', { type: 'close', code, reason, wasClean: true }), 0);
  }

  private emit(type: string, event: any) {
    const handler = (this as any)['on' + type] as Listener | null;
    handler?.(event);
    for (const listener of [...(this.listeners.get(type) ?? [])]) listener(event);
  }
}

/** Is this the socket address of the HoH environment? Default: any address on `hostname`. */
export function socketMatcher(hostname: string): (url: string) => boolean {
  return url => {
    try { return new URL(url).hostname === hostname; } catch { return false; }
  };
}

/**
 * Replace `globalThis.WebSocket` so that sockets for the HoH environment open in memory. Any other
 * address is refused (the window has no network of its own). Returns a function that restores the
 * previous constructor.
 */
export function installHohWebSocket(server: WireServer, isHohAddress: (url: string) => boolean, target: any = globalThis): () => void {
  const previous = target.WebSocket;
  class RoutedWebSocket extends HohWebSocket {
    constructor(url: string | URL) {
      const address = String(url);
      if (!isHohAddress(address)) throw new Error(`No network socket in HoH: ${address}`);
      super(address, server);
    }
  }
  target.WebSocket = RoutedWebSocket;
  return () => { target.WebSocket = previous; };
}
