// The HTTP side of the HoH environment, for T3's web client in browser mode (no desktopBridge).
// Before its socket opens, T3 reads `GET /api/auth/session` (the root route waits for it) and
// `GET /.well-known/t3/environment` (the primary environment's descriptor); everything else it asks
// over HTTP has a socket fallback, so it is answered 503 and the socket carries it.
// Addresses outside the HoH environment go to the previous fetch unchanged (bundle assets).
import { environmentDescriptor, type EnvironmentPlatform } from './config.ts';

export const HOH_SCOPES = [
  'orchestration:read', 'orchestration:operate', 'settings:write', 'providers:manage', 'filesystem:read', 'diagnostics:read',
] as const;

export function authSessionState() {
  return {
    authenticated: true,
    auth: {
      policy: 'desktop-managed-local',
      bootstrapMethods: ['desktop-bootstrap'],
      sessionMethods: ['browser-session-cookie'],
      sessionCookieName: 'hoh_session',
    },
    scopes: [...HOH_SCOPES],
    sessionMethod: 'browser-session-cookie',
  };
}

const json = (status: number, body: unknown) =>
  new Response(body === undefined ? null : JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });

/** The answer of the HoH environment to one HTTP request. */
export function hohHttpResponse(method: string, pathname: string, platform?: EnvironmentPlatform): Response {
  if (method === 'GET' && pathname === '/api/auth/session') return json(200, authSessionState());
  if (method === 'GET' && pathname === '/.well-known/t3/environment') return json(200, environmentDescriptor(platform));
  // Client tracing: accepted and dropped, nothing leaves the window.
  if (pathname.startsWith('/api/observability/')) return json(200, {});
  // Shell and thread snapshots, uploads, previews: the socket has them (or the feature is hidden).
  return new Response(null, { status: 503 });
}

/** Replace `globalThis.fetch` for the HoH environment's origin. Returns a function that restores it. */
export function installHohFetch(
  isHohAddress: (url: string) => boolean,
  target: any = globalThis,
  readHistory?: (threadId: string, cursor?: number) => Promise<unknown>,
  platform?: EnvironmentPlatform,
): () => void {
  const previous = target.fetch as typeof fetch;
  target.fetch = (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    if (!isHohAddress(url)) return previous(input as RequestInfo, init);
    const method = (init?.method ?? (typeof input === 'object' && 'method' in input ? input.method : 'GET')).toUpperCase();
    const address = new URL(url);
    const historyRoute = method === 'GET' && /^\/api\/orchestration\/threads\/([^/]+)\/(bounded|history)$/.exec(address.pathname);
    if (historyRoute && readHistory) {
      const [, pathId, type] = historyRoute;
      const input = address.searchParams.get('cursor') ?? '';
      const cursor = type === 'history' && /^\d+$/.test(input) ? Number(input) : undefined;
      if (type === 'history' && (cursor === undefined || !Number.isSafeInteger(cursor))) return Promise.resolve(new Response(null, { status: 400 }));
      return Promise.resolve().then(() => readHistory(decodeURIComponent(pathId), cursor))
        .then(result => result === undefined ? new Response(null, { status: 503 }) : json(200, result),
          () => new Response(null, { status: 503 }));
    }
    return Promise.resolve(hohHttpResponse(method, address.pathname, platform));
  };
  return () => { target.fetch = previous; };
}

/** What the window answers for one request T3 makes with XMLHttpRequest to the HoH environment. */
export type HohXhrReceiver = (method: string, pathname: string, contentType: string, body: unknown) => Promise<{ status: number }>;

/**
 * T3 sends a picture's bytes with XMLHttpRequest to the address `attachments.createUploadUrl` gave
 * (web/lib/attachmentUploadQueue.ts `uploadBytes`: `open('POST')`, `Content-Type`, `send(file)`, then `load` and
 * `status`). A request to the HoH environment's origin is answered in the window by `receive`, nothing goes on the
 * network; any other request is the platform's own. Returns a function that restores it.
 */
export function installHohXhr(isHohAddress: (url: string) => boolean, receive: HohXhrReceiver, target: any = globalThis): () => void {
  const Previous = target.XMLHttpRequest;
  if (typeof Previous !== 'function') return () => {};
  const event = (type: string) => (typeof ProgressEvent === 'function' ? new ProgressEvent(type) : new Event(type));
  class HohXhr extends Previous {
    #local: { method: string; url: string; contentType: string; status: number; readyState: number; done: boolean } | null = null;
    open(method: string, url: string | URL, ...rest: unknown[]) {
      const href = String(url);
      if (!isHohAddress(href)) {
        this.#local = null;
        return super.open(method, url, ...rest);
      }
      this.#local = { method: String(method).toUpperCase(), url: href, contentType: '', status: 0, readyState: 1, done: false };
    }
    setRequestHeader(name: string, value: string) {
      if (!this.#local) return super.setRequestHeader(name, value);
      if (String(name).toLowerCase() === 'content-type') this.#local.contentType = String(value);
    }
    get status() { return this.#local ? this.#local.status : super.status; }
    get readyState() { return this.#local ? this.#local.readyState : super.readyState; }
    abort() {
      const local = this.#local;
      if (!local) return super.abort();
      if (local.done) return;
      local.done = true; local.readyState = 4;
      this.dispatchEvent(event('abort')); this.dispatchEvent(event('loadend'));
    }
    send(body?: unknown) {
      const local = this.#local;
      if (!local) return super.send(body);
      const finish = (type: string, status: number) => {
        if (local.done) return;
        local.done = true; local.status = status; local.readyState = 4;
        this.dispatchEvent(new Event('readystatechange')); this.dispatchEvent(event(type)); this.dispatchEvent(event('loadend'));
      };
      receive(local.method, new URL(local.url).pathname, local.contentType, body).then(
        answer => finish('load', answer.status),
        () => finish('error', 0),
      );
    }
  }
  target.XMLHttpRequest = HohXhr;
  return () => { target.XMLHttpRequest = Previous; };
}
