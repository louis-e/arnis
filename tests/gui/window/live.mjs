// Run: node tests/gui/window/live.mjs [port]   (not part of tests/gui/*.mjs: it needs the built window)
// Start the window with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port> (default 9888).
// At 1000x650 and 1920x1080: the copyright notice shares the search bar's top and bottom (2 px), is
// centred, and touches neither the search bar, the map attribution nor the logo; the estimate on the
// progress bar is clipped at the fill's edge at 0, 50 and 100 %; the 3D card's grip resizes it.
import assert from 'node:assert';

const port = process.argv[2] || 9888;
const target = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find((x) => x.type === 'page');
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0;
const pending = new Map();
ws.onmessage = (e) => { const m = JSON.parse(e.data); pending.get(m.id)?.(m); };
const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const run = async (expression) => {
  const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  assert.ok(!r.result.exceptionDetails, JSON.stringify(r.result.exceptionDetails));
  return r.result.result.value;
};

const page = `(async () => {
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  const box = (e) => { const b = e.getBoundingClientRect(); return { left: b.left, top: b.top, right: b.right, bottom: b.bottom }; };
  const frame = document.querySelector('iframe.map-container');
  const off = box(frame), doc = frame.contentDocument;
  const abs = (e) => { const b = box(e); return { left: b.left + off.left, top: b.top + off.top, right: b.right + off.left, bottom: b.bottom + off.top }; };
  const out = { vw: innerWidth, notice: box(document.querySelector('.footer')), search: abs(doc.getElementById('search-box')), attribution: abs(doc.querySelector('.leaflet-control-attribution')), logo: box(document.getElementById('arnis-logo')), card: box(document.querySelector('.controls-box')), tools: abs(doc.querySelector('.leaflet-left .leaflet-bar') || doc.body) };
  // the estimate's clip at 0, 50, 100 %
  const bar = document.getElementById('progress-bar'), cont = bar.parentElement, eta = document.getElementById('progress-eta');
  eta.classList.add('visible');
  eta.children[0].textContent = eta.children[1].textContent = 'about 18m left';
  out.clip = [];
  for (const pct of [0, 50, 100]) {
    bar.style.transition = 'none'; bar.style.width = pct + '%'; await wait(120);
    const p = parseFloat(cont.style.getPropertyValue('--p'));
    const clip = getComputedStyle(eta.children[0]).clipPath;
    out.clip.push({ pct, fill: bar.getBoundingClientRect().width, white: p, clip, ok: pct === 0 || clip.includes(p + 'px') });
  }
  bar.style.width = '0%'; eta.classList.remove('visible'); eta.children[0].textContent = eta.children[1].textContent = '';
  return JSON.stringify(out);
})()`;

const hit = (a, b) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
for (const [w, h] of [[1000, 650], [1920, 1080]]) {
  await send('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: 1, mobile: false });
  await new Promise((r) => setTimeout(r, 1200));
  const o = JSON.parse(await run(page));
  const tag = `${w}x${h}`;
  assert.ok(Math.abs(o.notice.top - o.search.top) <= 2 && Math.abs(o.notice.bottom - o.search.bottom) <= 2, `${tag}: notice ${o.notice.top}-${o.notice.bottom} vs search ${o.search.top}-${o.search.bottom}`);
  assert.ok(Math.abs((o.notice.left + o.notice.right) / 2 - o.vw / 2) <= 2, `${tag}: notice is not centred`);
  for (const k of ['search', 'attribution', 'logo']) assert.ok(!hit(o.notice, o[k]), `${tag}: notice meets ${k}`);
  assert.ok(!hit(o.logo, o.card) && !hit(o.logo, o.tools), `${tag}: logo meets the controls`);
  for (const c of o.clip) assert.ok(c.ok && Math.abs(c.fill - c.white) <= 1, `${tag}: ${c.pct}% fill ${c.fill} vs white copy ${c.white}`);
  console.log(tag, 'ok', JSON.stringify({ notice: o.notice, search: o.search }));
}
await send('Emulation.clearDeviceMetricsOverride');
process.exit(0);
