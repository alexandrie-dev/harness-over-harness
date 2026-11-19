// Node-only component logic runner. Executes the authored TSX with deterministic
// hook commits, refs, clock and effects; this is not a DOM/browser renderer.
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve, dirname, extname } from 'node:path';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';
const require = createRequire(import.meta.url);
const jsx = (type, props) => ({ type, props: props ?? {} });
export const textOf = node => node == null || typeof node === 'boolean' ? '' : Array.isArray(node) ? node.map(textOf).join('') : typeof node === 'object' ? textOf(node.props?.children) : String(node);
export function deferred() { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }

export function componentHarness(file, name, mocks = {}, props = {}) {
  const slots = [], inputs = new Map(), listeners = new Map(), timers = new Map();
  let cursor = 0, dirty = true, tree, nodes = [], effects = [], clock = 100000, nextTimer = 0;
  const hooks = {
    useState(initial) {
      const index = cursor++;
      slots[index] ??= { kind: 'state', value: typeof initial === 'function' ? initial() : initial };
      return [slots[index].value, value => { slots[index].value = typeof value === 'function' ? value(slots[index].value) : value; dirty = true; }];
    },
    useRef(initial) { const index = cursor++; slots[index] ??= { kind: 'ref', value: { current: initial } }; return slots[index].value; },
    useEffect(fn, deps) {
      const index = cursor++, before = slots[index];
      if (!before || !deps || deps.some((value, i) => !Object.is(value, before.deps?.[i]))) {
        slots[index] = { kind: 'effect', deps, cleanup: before?.cleanup };
        effects.push(() => { slots[index].cleanup?.(); slots[index].cleanup = fn(); });
      }
    },
  };
  const window = {
    addEventListener(type, fn) { if (!listeners.has(type)) listeners.set(type, new Set()); listeners.get(type).add(fn); },
    removeEventListener(type, fn) { listeners.get(type)?.delete(fn); },
  };
  const modules = new Map();
  function load(path) {
    if (modules.has(path)) return modules.get(path);
    const module = { exports: {} }; modules.set(path, module.exports);
    const source = ts.transpileModule(readFileSync(path, 'utf8'), { fileName: path, compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX } }).outputText;
    const localRequire = id => {
      if (id === 'react') return hooks;
      if (id === 'react/jsx-runtime') return { jsx, jsxs: jsx, Fragment: 'fragment' };
      if (id in mocks) return mocks[id];
      if (id.endsWith('.css')) return {};
      if (id.startsWith('.')) return load(resolve(dirname(path), extname(id) ? id : id + '.tsx'));
      return require(id);
    };
    const setTimer = (fn, delay, repeat = false) => { const id = ++nextTimer; timers.set(id, { fn, at: clock + delay, delay, repeat }); return id; };
    runInNewContext(source, {
      exports: module.exports, module, require: localRequire, window, Error,
      Date: class extends Date { static now() { return clock; } },
      setTimeout: (fn, ms) => setTimer(fn, ms), clearTimeout: id => timers.delete(id),
      setInterval: (fn, ms) => setTimer(fn, ms, true), clearInterval: id => timers.delete(id),
    }, { filename: path });
    modules.set(path, module.exports); return module.exports;
  }
  const component = load(resolve(file))[name];
  function commit(node, disabled = false) {
    if (node == null || typeof node !== 'object') return;
    if (Array.isArray(node)) { node.forEach(child => commit(child, disabled)); return; }
    if (!node.props) return;
    if (typeof node.type === 'string') {
      node.disabled = disabled || !!node.props.disabled;
      node.focus = () => {};
      nodes.push(node);
      if (node.type === 'input') {
        const input = inputs.get(node.props.id) ?? { value: '' };
        if (node.props.value !== undefined) input.value = node.props.value;
        input.disabled = node.disabled; inputs.set(node.props.id, input);
        if (node.props.ref) node.props.ref.current = input;
      } else if (node.props.ref) node.props.ref.current = node;
    }
    commit(node.props.children, node.disabled ?? disabled);
  }
  function render() {
    while (dirty) {
      dirty = false; cursor = 0; effects = []; nodes = []; tree = component(props); commit(tree);
      const pending = effects; effects = []; pending.forEach(fn => fn());
    }
  }
  async function settle() { for (let i = 0; i < 12; i++) { await Promise.resolve(); render(); } }
  render();
  return {
    render, settle, slots, inputs, forceRender() { dirty = true; render(); }, exports: modules.get(resolve(file)),
    nodes: () => nodes, text: () => textOf(tree),
    button(label) { return nodes.find(node => node.type === 'button' && textOf(node) === label); },
    byId(id) { return nodes.find(node => node.props.id === id); },
    async click(label) { const node = this.button(label); if (!node) throw new Error('Missing button ' + label); if (!node.disabled) node.props.onClick?.(); await settle(); },
    async input(id, value) { const node = this.byId(id); if (node.disabled) throw new Error('Disabled input ' + id); const input = inputs.get(id); input.value = value; node.props.onChange?.({ target: input }); await settle(); },
    async event(type, event = {}) { for (const fn of [...listeners.get(type) ?? []]) fn({ preventDefault() {}, ...event }); await settle(); },
    listeners(type) { return [...listeners.get(type) ?? []]; },
    now: () => clock,
    async advance(ms, tick = true) {
      clock += ms;
      if (tick) for (const [id, timer] of [...timers]) if (timer.at <= clock) {
        if (timer.repeat) timer.at = clock + timer.delay; else timers.delete(id);
        timer.fn();
      }
      await settle();
    },
    unmount() { for (const slot of slots) if (slot.kind === 'effect') slot.cleanup?.(); },
  };
}
