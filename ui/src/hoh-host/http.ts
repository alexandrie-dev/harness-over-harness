// The HTTP side of the HoH environment, for T3's web client in browser mode (no desktopBridge).
// Before its socket opens, T3 reads `GET /api/auth/session` (the root route waits for it) and
// `GET /.well-known/t3/environment` (the primary environment's descriptor); everything else it asks
// over HTTP has a socket fallback, so it is answered 503 and the socket carries it.
// Addresses outside the HoH environment go to the previous fetch unchanged (bundle assets).
import { HOH_ENVIRONMENT_ID, HOH_LABEL, ORCHESTRATION_PROTOCOL_VERSION } from './config.ts';

export const HOH_SCOPES = [
  'orchestration:read', 'orchestration:operate', 'settings:write', 'providers:manage', 'filesystem:read', 'filesystem:write',
] as const;

export function environmentDescriptor() {
  return {
    environmentId: HOH_ENVIRONMENT_ID,
    label: HOH_LABEL,
    platform: { os: 'linux', arch: 'x64' },
    serverVersion: '0.0.0-hoh',
    orchestrationProtocolVersion: ORCHESTRATION_PROTOCOL_VERSION,
    capabilities: { repositoryIdentity: false, connectionProbe: true, threadForceStop: true, inlineMessageContext: true },
  };
}

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
export function hohHttpResponse(method: string, pathname: string): Response {
  if (method === 'GET' && pathname === '/api/auth/session') return json(200, authSessionState());
  if (method === 'GET' && pathname === '/.well-known/t3/environment') return json(200, environmentDescriptor());
  // Client tracing: accepted and dropped, nothing leaves the window.
  if (pathname.startsWith('/api/observability/')) return json(200, {});
  // Shell and thread snapshots, uploads, previews: the socket has them (or the feature is hidden).
  return new Response(null, { status: 503 });
}

/** Replace `globalThis.fetch` for the HoH environment's origin. Returns a function that restores it. */
export function installHohFetch(isHohAddress: (url: string) => boolean, target: any = globalThis): () => void {
  const previous = target.fetch as typeof fetch;
  target.fetch = (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    if (!isHohAddress(url)) return previous(input as RequestInfo, init);
    const method = (init?.method ?? (typeof input === 'object' && 'method' in input ? input.method : 'GET')).toUpperCase();
    return Promise.resolve(hohHttpResponse(method, new URL(url).pathname));
  };
  return () => { target.fetch = previous; };
}
