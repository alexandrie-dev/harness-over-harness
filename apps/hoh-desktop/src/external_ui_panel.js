;(() => {
  let items = Object.freeze([]);
  window.addEventListener('hoh-review-queue', (event) => {
    items = Object.freeze(event.detail.map((item) => Object.freeze(item)));
  });
  const sdk = Object.freeze({
    version: 2,
    reviewQueue: Object.freeze({
      get: () => items,
      subscribe: (listener) => {
        const update = () => listener(items);
        window.addEventListener('hoh-review-queue', update);
        update();
        return () => window.removeEventListener('hoh-review-queue', update);
      },
    }),
    openChat: (threadId) => {
      if (items.some((item) => item.threadId === threadId)) {
        window.webkit.messageHandlers.hohOpenChat.postMessage(threadId);
      }
    },
  });
  mount(document.getElementById('hoh-plugin-root'), sdk);
})();
