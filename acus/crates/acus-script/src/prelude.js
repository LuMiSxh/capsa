const __prints = [];
const __tag = (items, kind, text) => {
  Object.defineProperty(items, 'kind', { value: kind, enumerable: false, writable: true });
  if (typeof text === 'string') {
    Object.defineProperty(items, 'text', { value: text, enumerable: false, writable: true });
  }
  return items;
};
const __lift = (v) =>
  v !== null && typeof v === 'object' && !Array.isArray(v) && typeof v.kind === 'string' && Array.isArray(v.items)
    ? __tag(v.items, v.kind, v.text)
    : v;
const __ser = (v) =>
  Array.isArray(v) && v.kind
    ? { kind: v.kind, items: [...v], ...(typeof v.text === 'string' ? { text: v.text } : {}) }
    : v;
const __method = (name, fn) =>
  Object.defineProperty(Array.prototype, name, { value: fn, enumerable: false, writable: true });
const __keep = (arr, items) => (arr.kind ? __tag(items, arr.kind) : items);
__method('head', function (n) { return __keep(this, this.slice(0, n)); });
__method('tail', function (n) { return __keep(this, n > 0 ? this.slice(-n) : []); });
__method('grep', function (re) {
  const rx = re instanceof RegExp ? re : new RegExp(re);
  const text = (x) => (typeof x === 'string' ? x : x && typeof x.text === 'string' ? x.text : '');
  return __keep(this, this.filter((x) => { rx.lastIndex = 0; return rx.test(text(x)); }));
});
__method('paths', function () {
  return [...new Set(this.map((x) => x && x.path).filter((p) => typeof p === 'string'))];
});
const print = (...values) => { __prints.push(...values); };
const parse = { toml: (text) => parse_toml(text), yaml: (text) => parse_yaml(text) };
const __waiting = new Map();
let __next = 0;
const __install = (names) => {
  for (const name of names) {
    globalThis[name] = (...args) => {
      const site = new Error().stack;
      return new Promise((resolve, reject) => {
        const id = ++__next;
        __waiting.set(id, { resolve, reject, site, call: name, args });
        __host(id, name, JSON.stringify(args));
      });
    };
  }
};
globalThis.__settle = (id, ok, text) => {
  const { resolve, reject, site, call, args } = __waiting.get(id);
  __waiting.delete(id);
  if (ok) {
    resolve(__lift(JSON.parse(text)));
    return;
  }
  const { message, hint } = JSON.parse(text);
  const error = new Error(message);
  error.stack = site;
  error.call = call;
  error.args = args;
  if (hint !== null) error.hint = hint;
  reject(error);
};
const __failure = (e) => ({
  ok: false,
  error: e?.message ?? String(e),
  hint: e?.hint ?? null,
  call: e?.call ?? null,
});
// Resolves to every result in order; a rejected entry becomes a failure object instead.
const all = (promises) => Promise.all(Array.from(promises, (p) => Promise.resolve(p).catch(__failure)));
// Calls `fn` up to `times` times in all, waiting `delayMs` between attempts.
const retry = async (fn, { times = 3, delayMs = 200 } = {}) => {
  for (let attempt = 1; ; attempt++) {
    try {
      return await fn();
    } catch (error) {
      if (attempt >= times) throw error;
      await sleep(delayMs);
    }
  }
};
const __finish = (r) =>
  JSON.stringify({ prints: __prints.map(__ser), defined: r !== undefined, result: r === undefined ? null : __ser(r) });
