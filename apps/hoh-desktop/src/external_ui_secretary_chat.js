;{
  const encoder = new TextEncoder();
  let nextId = 0;
  let pending;
  window.addEventListener('hoh-secretary-chat-reply', ({ detail }) => {
    if (!pending || detail?.requestId !== pending.id) return;
    const request = pending;
    pending = undefined;
    window.clearTimeout(request.timeout);
    if (typeof detail.error === 'string') request.reject(new Error(detail.error));
    else request.resolve(typeof detail.value === 'string' ? detail.value : null);
  });

  function call(action, input) {
    if (pending) return Promise.reject(new Error('A Cabinet chat action is already in progress.'));
    if (action === 'openChat' && (
      typeof input?.revision !== 'string' || !input.revision || encoder.encode(input.revision).length > 128 ||
      typeof input.instructions !== 'string' || !input.instructions.trim() ||
      typeof input.context !== 'object' || input.context === null || Array.isArray(input.context)
    )) {
      return Promise.reject(new Error('The Cabinet chat context is invalid.'));
    }
    const id = nextId = (nextId % 0xffffffff) + 1;
    return new Promise((resolve, reject) => {
      const payload = JSON.stringify({ requestId: id, action, input });
      if (encoder.encode(payload).length > 128 * 1024) {
        reject(new Error('The Cabinet chat context is too large.'));
        return;
      }
      const timeout = window.setTimeout(() => {
        if (pending?.id === id) pending = undefined;
        reject(new Error('The HoH chat action timed out.'));
      }, 16000);
      pending = { id, resolve, reject, timeout };
      try {
        window.webkit.messageHandlers.hohSecretaryChat.postMessage(payload);
      } catch (error) {
        window.clearTimeout(timeout);
        pending = undefined;
        reject(error);
      }
    });
  }

  mount(document.getElementById('hoh-plugin-root'), Object.freeze({
    version: 5,
    secretary: Object.freeze({
      async openChat(input) { await call('openChat', input); },
      getProposal() { return call('getProposal'); },
    }),
  }));
}
