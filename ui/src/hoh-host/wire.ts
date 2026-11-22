// The server end of T3's effect/rpc protocol, spoken in its JSON wire form (RpcSerialization.json).
// The client side is T3's own `RpcClient.makeProtocolSocket`, unchanged; frames come from it through
// `HohWebSocket` (socket.ts). Shapes of the frames: effect/rpc RpcMessage, as T3's session tests write
// them (packages/client-runtime/src/rpc/session.test.ts): Request / Ack / Interrupt / Ping / Eof in,
// Chunk / Exit / Pong out. Values are already in their encoded (JSON) form.
import { ALL_METHODS, NO_AUTH_ERROR_METHODS, STREAM_METHODS } from './methods.ts';

/** A typed failure of a method: sent as `Fail` with this error (it must be a member of the method's error union). */
export class RpcFailure extends Error {
  constructor(readonly error: { readonly _tag: string; readonly message?: string; readonly [key: string]: unknown }) {
    super(error.message ?? error._tag);
  }
}

export interface StreamSink {
  emit(...values: unknown[]): void;
  end(): void;
  fail(error: unknown): void;
}

export type UnaryHandler = (payload: any) => Promise<unknown> | unknown;
/** Returns what to run when the client stops listening (interrupt or socket close). */
export type StreamHandler = (payload: any, sink: StreamSink) => (() => void) | void;
export interface Handlers {
  readonly unary: Readonly<Record<string, UnaryHandler>>;
  readonly stream: Readonly<Record<string, StreamHandler>>;
}

export interface WireConnection {
  receive(frame: string): void;
  close(): void;
}

const NOT_SUPPORTED = 'Not available in HoH.';

function exitFailure(error: unknown, typed: boolean) {
  return typed
    ? { _tag: 'Failure', cause: [{ _tag: 'Fail', error }] }
    : { _tag: 'Failure', cause: [{ _tag: 'Die', defect: { name: 'Error', message: error instanceof Error ? error.message : String(error) } }] };
}

/** The typed "not supported" answer of a method that has no handler here. */
export function notSupported(method: string): { typed: boolean; error: unknown } {
  if (!ALL_METHODS.has(method) || NO_AUTH_ERROR_METHODS.has(method)) return { typed: false, error: new Error(NOT_SUPPORTED) };
  return { typed: true, error: { _tag: 'EnvironmentAuthorizationError', message: NOT_SUPPORTED, requiredScope: 'orchestration:operate' } };
}

export interface WireServerOptions {
  /** Called for every request, after the handler is chosen. For diagnostics only. */
  readonly onRequest?: (method: string, payload: unknown) => void;
  readonly onError?: (method: string, error: unknown) => void;
}

export class WireServer {
  constructor(private readonly handlers: Handlers, private readonly options: WireServerOptions = {}) {}

  /** One socket. `send` delivers a frame to the client. */
  connect(send: (frame: string) => void): WireConnection {
    const open = new Map<string, () => void>();
    let closed = false;
    const write = (message: unknown) => { if (!closed) send(JSON.stringify(message)); };

    // Replies carry the request id exactly as it came (effect encodes its bigint ids as strings).
    const unary = (id: unknown, method: string, payload: unknown) => {
      const handler = this.handlers.unary[method];
      const run = async () => {
        if (!handler) {
          const { typed, error } = notSupported(method);
          return { _tag: 'Exit', requestId: id, exit: exitFailure(error, typed) };
        }
        try {
          const value = await handler(payload);
          // effect's JSON codec writes a void result as null; a missing key fails to decode.
          return { _tag: 'Exit', requestId: id, exit: { _tag: 'Success', value: value === undefined ? null : value } };
        } catch (error) {
          this.options.onError?.(method, error);
          return { _tag: 'Exit', requestId: id, exit: exitFailure(error instanceof RpcFailure ? error.error : error, error instanceof RpcFailure) };
        }
      };
      void run().then(write);
    };

    const stream = (id: unknown, method: string, payload: unknown) => {
      const key = String(id);
      const handler = this.handlers.stream[method];
      // Hidden features keep their subscriptions open and quiet: no data, no error, no retry loop.
      if (!handler) { open.set(key, () => {}); return; }
      let done = false;
      const finish = (message: unknown) => {
        if (done) return;
        done = true; open.delete(key); write(message);
      };
      const sink: StreamSink = {
        emit: (...values) => { if (!done && values.length) write({ _tag: 'Chunk', requestId: id, values }); },
        end: () => finish({ _tag: 'Exit', requestId: id, exit: { _tag: 'Success', value: null } }),
        fail: error => {
          this.options.onError?.(method, error);
          finish({ _tag: 'Exit', requestId: id, exit: exitFailure(error instanceof RpcFailure ? error.error : error, error instanceof RpcFailure) });
        },
      };
      open.set(key, () => { done = true; });
      try {
        const stop = handler(payload, sink);
        if (stop) {
          if (done) stop();
          else open.set(key, () => { done = true; stop(); });
        }
      } catch (error) { sink.fail(error); }
    };

    const receiveOne = (message: any) => {
      switch (message?._tag) {
        case 'Ping': write({ _tag: 'Pong' }); return;
        case 'Request': {
          const id = message.id; const method = String(message.tag);
          this.options.onRequest?.(method, message.payload);
          if (STREAM_METHODS.has(method) || this.handlers.stream[method]) stream(id, method, message.payload);
          else unary(id, method, message.payload);
          return;
        }
        case 'Interrupt': {
          const id = String(message.requestId);
          const stop = open.get(id); open.delete(id); stop?.();
          return;
        }
        default: return; // Ack and Eof need no answer: chunks are not held back for acks.
      }
    };

    return {
      receive: frame => {
        if (closed) return;
        let parsed: unknown;
        try { parsed = JSON.parse(frame); } catch { return; }
        for (const message of Array.isArray(parsed) ? parsed : [parsed]) receiveOne(message);
      },
      close: () => {
        if (closed) return;
        closed = true;
        for (const stop of open.values()) stop();
        open.clear();
      },
    };
  }
}
