// The client end of effect/rpc's JSON frames, written out as T3's session tests show them
// (packages/client-runtime/src/rpc/session.test.ts), over the host's in-memory socket.
export class WireClient {
  constructor(HohWebSocket, server, url = 'ws://hoh.local/ws?frameParts=1') {
    this.socket = new HohWebSocket(url, server);
    this.next = 0; this.exits = new Map(); this.streams = new Map(); this.frames = [];
    this.opened = new Promise(resolve => this.socket.addEventListener('open', resolve));
    this.socket.addEventListener('message', event => this.receive(JSON.parse(event.data)));
  }
  receive(message) {
    this.frames.push(message);
    if (message._tag === 'Chunk') {
      const stream = this.streams.get(message.requestId);
      stream?.values.push(...message.values); stream?.wake();
      if (stream) this.send({ _tag: 'Ack', requestId: message.requestId });
    }
    if (message._tag === 'Exit') {
      const stream = this.streams.get(message.requestId);
      if (stream) { stream.exit = message.exit; stream.wake(); return; }
      this.exits.get(message.requestId)?.(message.exit);
    }
  }
  send(message) { this.socket.send(JSON.stringify(message)); }
  async request(tag, payload = {}) {
    await this.opened;
    const id = String(++this.next);
    const exit = new Promise(resolve => this.exits.set(id, resolve));
    this.send({ _tag: 'Request', id, tag, payload, headers: [] });
    return exit;
  }
  /** Success value of a unary call, or throws with the exit. */
  async call(tag, payload = {}) {
    const exit = await this.request(tag, payload);
    if (exit._tag !== 'Success') throw Object.assign(new Error(`${tag} failed: ${JSON.stringify(exit.cause)}`), { exit });
    return exit.value;
  }
  async stream(tag, payload = {}) {
    await this.opened;
    const id = String(++this.next);
    const stream = {
      id, values: [], exit: undefined, waiters: [],
      wake() { for (const waiter of this.waiters.splice(0)) waiter(); },
      /** Resolves with the first value (from `from` on) that matches. */
      async until(match, from = 0, ms = 2000) {
        const deadline = Date.now() + ms;
        for (;;) {
          const found = this.values.slice(from).find(match);
          if (found) return found;
          if (Date.now() > deadline) throw new Error(`no matching value in ${tag}: ${JSON.stringify(this.values.slice(-3)).slice(0, 600)}`);
          await new Promise(resolve => { this.waiters.push(resolve); setTimeout(resolve, 20); });
        }
      },
    };
    this.streams.set(id, stream);
    this.send({ _tag: 'Request', id, tag, payload, headers: [] });
    return stream;
  }
  interrupt(stream) { this.send({ _tag: 'Interrupt', requestId: stream.id, interruptors: [] }); }
  close() { this.socket.close(); }
}

export const tick = (ms = 0) => new Promise(resolve => setTimeout(resolve, ms));
