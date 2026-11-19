const finish = (pass, detail) => {
  document.title = `HOH_APPROVAL_PROBE:${JSON.stringify({ pass, detail })}`;
};

const timer = setTimeout(() => finish(false, 'timeout'), 20_000);
const invoke = (method, params) => window.__TAURI_INTERNALS__.invoke('engine_request', { method, params });

const waitUntilReady = async () => {
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    try {
      await invoke('hoh/settings/get', {});
      return;
    } catch {
      await new Promise(resolve => setTimeout(resolve, 50));
    }
  }
  throw new Error('engine did not become ready');
};

try {
  await waitUntilReady();
  await invoke('hoh/model/key/set', {
    apiKey: 'headless-probe-secret',
    baseUrl: 'https://api.example.test/v1',
  });
  clearTimeout(timer);
  finish(false, 'privileged request unexpectedly completed');
} catch {
  clearTimeout(timer);
  finish(true, 'approval window close rejected preflight');
}
