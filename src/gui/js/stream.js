// Stream mode window.
//
// Polls `gui_stream_status` once a second and pushes the result into four bits
// of text plus one CSS custom property. There is deliberately no log pane, no
// anchor list and no controls: the window exists so that a glance at the
// screen behind Minecraft answers "is Arnis live?".
//
// Closing the window is what stops stream mode; that is handled on the Rust
// side by the window's Destroyed handler, so there is nothing to do here.

const invoke =
  window.__TAURI__ && window.__TAURI__.core
    ? window.__TAURI__.core.invoke
    : null;

const POLL_MS = 1000;

// Ring tempo per state. Faster means busier; stopped keeps the slow value so
// that the paused rings do not jump when the server comes back.
const PULSE_INTERVAL = {
  waiting: '2.4s',
  connected: '1.6s',
  busy: '0.9s',
  stopped: '2.4s',
};

const addressEl = document.getElementById('stream-address');
const clientEl = document.getElementById('stream-client');
const countersEl = document.getElementById('stream-counters');

function setState(state) {
  if (document.body.dataset.state !== state) {
    document.body.dataset.state = state;
  }
  const interval = PULSE_INTERVAL[state] || PULSE_INTERVAL.waiting;
  document.body.style.setProperty('--pulse-interval', interval);
}

function setText(el, text) {
  if (el && el.textContent !== text) {
    el.textContent = text;
  }
}

function count(n) {
  return Number(n || 0).toLocaleString();
}

// Cache hit rate over every chunk/column request answered so far. Before the
// first request there is nothing to average, so say so rather than show 0%.
function cacheHitText(status) {
  const hits = Number(status.cacheHits || 0);
  const misses = Number(status.cacheMisses || 0);
  const total = hits + misses;
  if (total === 0) return 'no requests yet';
  return `${Math.round((hits / total) * 100)}% cache hits`;
}

function render(status) {
  setState(
    status.requestsInFlight > 0
      ? 'busy'
      : status.clientConnected
        ? 'connected'
        : 'waiting'
  );

  setText(addressEl, `127.0.0.1:${status.port}`);

  if (status.clientConnected) {
    const name = status.clientName || 'a client';
    const version = status.clientVersion ? ` ${status.clientVersion}` : '';
    setText(clientEl, `Connected: ${name}${version}`);
  } else {
    setText(clientEl, 'Waiting for Minecraft to connect...');
  }

  setText(
    countersEl,
    `${count(status.chunksServed)} chunks · ` +
      `${count(status.tilesGenerated)} tiles · ` +
      cacheHitText(status)
  );
}

// The server is gone (stopped, or never started). The window is about to be
// closed by the Rust side in the normal case, so this is mostly the brief
// moment in between — and the honest display if it ever is not.
function renderStopped(message) {
  setState('stopped');
  setText(clientEl, message);
  setText(countersEl, '');
}

async function poll() {
  if (!invoke) {
    renderStopped('Not running inside Arnis.');
    return;
  }
  try {
    render(await invoke('gui_stream_status'));
  } catch (error) {
    renderStopped(
      typeof error === 'string' ? error : 'Stream mode is not running.'
    );
  }
}

poll();
setInterval(poll, POLL_MS);
