;(() => {
  const encoder = new TextEncoder();
  let nextId = 0;
  let pending;

  function failure(code, message) {
    return Object.assign(new Error(message), { code });
  }

  window.addEventListener('hoh-secretary-result', ({ detail }) => {
    if (!pending || detail?.id !== pending.id) return;
    const request = pending;
    pending = undefined;
    window.clearTimeout(request.timeout);
    if (typeof detail.text === 'string') {
      request.resolve(detail.text);
    } else {
      request.reject(failure(detail.error.code, detail.error.message));
    }
  });

  const sdk = Object.freeze({
    version: 4,
    secretary: Object.freeze({
      async propose(params) {
        if (pending) throw failure('busy', 'Дождитесь ответа на предыдущий запрос.');
        const { message, instructions, context } = params ?? {};
        if (typeof message !== 'string' || !message.trim() || encoder.encode(message).length > 4096
            || typeof instructions !== 'string' || !instructions.trim()
            || encoder.encode(instructions).length > 12288
            || !context || typeof context !== 'object' || Array.isArray(context)) {
          throw failure('invalid_request', 'Некорректный запрос.');
        }
        const id = nextId = (nextId + 1) >>> 0;
        let wire;
        try {
          wire = JSON.stringify({ id, params: { message, instructions, context } });
        } catch {
          throw failure('invalid_request', 'Контекст запроса должен быть JSON-объектом.');
        }
        if (wire.length > 131072 || encoder.encode(wire).length > 131072) {
          throw failure('invalid_request', 'Контекст запроса слишком большой.');
        }
        return new Promise((resolve, reject) => {
          const timeout = window.setTimeout(() => {
            pending = undefined;
            reject(failure('timeout', 'HoH не ответил вовремя. Повторите запрос.'));
          }, 315000);
          pending = { id, resolve, reject, timeout };
          try {
            window.webkit.messageHandlers.hohSecretaryPropose.postMessage(wire);
          } catch {
            pending = undefined;
            window.clearTimeout(timeout);
            reject(failure('unavailable', 'Секретарь недоступен в этой версии HoH.'));
          }
        });
      },
    }),
  });
  mount(document.getElementById('hoh-plugin-root'), sdk);
})();
